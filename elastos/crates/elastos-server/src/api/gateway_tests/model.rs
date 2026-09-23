use super::*;
use crate::api::gateway::gateway_home_token;
use crate::api::gateway::gateway_provider_proxy::project_model_provider_response;
use axum::body::Body;
use elastos_model_contract::{
    model_input_hash, RuntimeAccessBinding, RuntimeCreateBinding, RUNTIME_ACCESS_BINDING_SCHEMA,
    RUNTIME_CREATE_BINDING_SCHEMA,
};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
struct RecordingModelProvider {
    requests: Arc<TokioMutex<Vec<Value>>>,
    response: Arc<TokioMutex<Value>>,
}

impl Default for RecordingModelProvider {
    fn default() -> Self {
        Self {
            requests: Arc::new(TokioMutex::new(Vec::new())),
            response: Arc::new(TokioMutex::new(json!({ "status": "ok" }))),
        }
    }
}

#[async_trait::async_trait]
impl Provider for RecordingModelProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "resource requests are not used in this test".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["model"]
    }

    fn name(&self) -> &'static str {
        "recording-model-provider"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        self.requests.lock().await.push(request.clone());
        Ok(self.response.lock().await.clone())
    }
}

async fn model_test_state(
    cache_dir: &std::path::Path,
    provider: RecordingModelProvider,
) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("model", Arc::new(provider))
        .await
        .unwrap();
    GatewayState {
        provider_registry: Some(registry),
        collaboration_chat_product_port: None,
        collaboration_presence_product_port: None,
        carrier_endpoint: None,
        collaboration_discovery_service: None,
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

fn assistant_auth_grant(
    data_dir: &std::path::Path,
    authority: &TestPasskeyAuthority,
) -> AuthSessionGrantV1 {
    let now = crate::auth::now_ts();
    let grant = AuthSessionGrantV1 {
        schema: AuthSessionGrantV1::SCHEMA.to_string(),
        grant_id: format!("grant:{}", gateway_home_token::uuid_like_token()),
        session_id: format!("auth:{}", gateway_home_token::uuid_like_token()),
        principal_id: authority.principal_id.clone(),
        proof_binding_id: authority.proof_binding_id.clone(),
        issued_at: now,
        expires_at: now + 12 * 60 * 60,
        apps: vec!["assistant".to_string()],
    };
    crate::auth::store_session_grant(data_dir, grant.clone()).unwrap();
    grant
}

fn post_model(token: String, op: &str, body: Value) -> Request<Body> {
    test_browser_request("localhost:61180", "null")
        .method("POST")
        .uri(format!("/api/provider/model/{op}"))
        .header("x-elastos-home-token", token)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn get_assistant_workspace(token: String) -> Request<Body> {
    test_browser_request("localhost:61180", "null")
        .method("GET")
        .uri("/api/apps/assistant/workspace")
        .header("x-elastos-home-token", token)
        .body(Body::empty())
        .unwrap()
}

fn put_assistant_workspace(token: String, body: Value) -> Request<Body> {
    test_browser_request("localhost:61180", "null")
        .method("PUT")
        .uri("/api/apps/assistant/workspace")
        .header("x-elastos-home-token", token)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn assistant_workspace_object(
    data_dir: &std::path::Path,
    principal_id: &str,
) -> (String, std::path::PathBuf) {
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    let object_uri = format!("{localhost_root}/.AppData/ElastOS/Assistant/workspace.json");
    let object_path =
        elastos_common::localhost::rooted_localhost_fs_path(data_dir, &object_uri).unwrap();
    (object_uri, object_path)
}

fn sample_workspace_put(if_revision: u64) -> Value {
    json!({
        "schema": "elastos.assistant.workspace/v1",
        "if_revision": if_revision,
        "sessions": [
            {
                "id": "session-1",
                "title": "First session",
                "mode": "chat",
                "pinned": true,
                "messages": [
                    { "role": "user", "content": "hello" },
                    {
                        "role": "assistant",
                        "content": "hi",
                        "run_id": format!("run:sha256:{}", "a".repeat(64))
                    }
                ]
            },
            {
                "id": "session-2",
                "title": "Build session",
                "mode": "build",
                "pinned": false,
                "messages": []
            }
        ],
        "draft": "Draft note",
        "selected_offer_id": "offer:sample-model"
    })
}

fn write_protected_workspace_fixture(
    data_dir: &std::path::Path,
    principal_id: &str,
    body: &Value,
) -> std::path::PathBuf {
    let protection = crate::auth::store_test_principal_root_protection(data_dir, principal_id);
    let (object_uri, object_path) = assistant_workspace_object(data_dir, principal_id);
    crate::auth::write_protected_principal_root_object(
        data_dir,
        principal_id,
        &protection.localhost_root,
        &object_uri,
        &object_path,
        &serde_json::to_vec_pretty(body).unwrap(),
    )
    .unwrap();
    object_path
}

async fn response_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn non_assistant_capsule_cannot_invoke_model_provider() {
    let dir = tempfile::tempdir().unwrap();
    let provider = RecordingModelProvider::default();
    let app = gateway_router(model_test_state(dir.path(), provider.clone()).await);
    let token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();

    let response = app
        .oneshot(post_model(token, "offers_list", json!({})))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(provider.requests.lock().await.is_empty());
}

#[tokio::test]
async fn marketplace_reads_model_offers_and_services_without_run_or_grant_authority() {
    let dir = tempfile::tempdir().unwrap();
    let provider = RecordingModelProvider::default();
    *provider.response.lock().await = json!({"status":"ok", "data":{"offers":[]}});
    let app = gateway_router(model_test_state(dir.path(), provider.clone()).await);
    let token = issue_home_launch_token(dir.path(), MARKETPLACE_CAPSULE_ID).unwrap();
    let response = app
        .clone()
        .oneshot(post_model(token.clone(), "offers_list", json!({})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    for operation in ["runs_create", "runs_get", "runs_events", "runs_cancel"] {
        let response = app
            .clone()
            .oneshot(post_model(token.clone(), operation, json!({})))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{operation}");
    }
    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/services/offers")
                .header("x-elastos-home-token", &token)
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"offer_id":"local:provider:model","section":"mine","selected":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        *provider.requests.lock().await,
        vec![json!({"op":"offers_list"})]
    );
}

#[tokio::test]
async fn assistant_cannot_invoke_unsupported_model_operation() {
    let dir = tempfile::tempdir().unwrap();
    let provider = RecordingModelProvider::default();
    let app = gateway_router(model_test_state(dir.path(), provider.clone()).await);
    let token = issue_home_launch_token(dir.path(), "assistant").unwrap();

    for operation in ["offer_get", "init"] {
        let response = app
            .clone()
            .oneshot(post_model(
                token.clone(),
                operation,
                json!({"config":{"extra":{"offers":[]}}}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    assert!(provider.requests.lock().await.is_empty());
}

#[tokio::test]
async fn model_runs_create_injects_verified_runtime_binding() {
    let dir = tempfile::tempdir().unwrap();
    let provider = RecordingModelProvider::default();
    let app = gateway_router(model_test_state(dir.path(), provider.clone()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let grant = assistant_auth_grant(dir.path(), &authority);
    let token = issue_home_launch_token_for_auth_grant(dir.path(), "assistant", &grant).unwrap();
    let input = json!({
        "messages": [{ "role": "user", "content": "hello" }]
    });

    let response = app
        .oneshot(post_model(
            token,
            "runs_create",
            json!({
                "offer_id": "offer:flash-chat:pair-a",
                "operation": "text.generate",
                "request_id": "request-1",
                "input": input,
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let request = requests[0].clone();
    assert_eq!(request["op"], "runs_create");
    assert_eq!(request["offer_id"], "offer:flash-chat:pair-a");
    assert_eq!(request["operation"], "text.generate");
    let binding: RuntimeCreateBinding =
        serde_json::from_value(request["runtime_binding"].clone()).unwrap();
    assert_eq!(binding.schema, RUNTIME_CREATE_BINDING_SCHEMA);
    assert_eq!(binding.principal_id, authority.principal_id);
    assert_eq!(binding.session_id, grant.session_id);
    assert_eq!(binding.capsule_id, "assistant");
    assert_eq!(binding.grant_id, grant.grant_id);
    assert_eq!(binding.request_id, "request-1");
    assert_eq!(binding.offer_id, "offer:flash-chat:pair-a");
    assert_eq!(binding.operation, "text.generate");
    assert_eq!(
        binding.input_hash,
        model_input_hash(&request["input"]).unwrap()
    );
}

#[tokio::test]
async fn home_agent_capsule_runs_bind_and_audit_to_its_own_capsule_id() {
    let dir = tempfile::tempdir().unwrap();
    let provider = RecordingModelProvider::default();
    let app = gateway_router(model_test_state(dir.path(), provider.clone()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let now = crate::auth::now_ts();
    let grant = AuthSessionGrantV1 {
        schema: AuthSessionGrantV1::SCHEMA.to_string(),
        grant_id: format!("grant:{}", gateway_home_token::uuid_like_token()),
        session_id: format!("auth:{}", gateway_home_token::uuid_like_token()),
        principal_id: authority.principal_id.clone(),
        proof_binding_id: authority.proof_binding_id.clone(),
        issued_at: now,
        expires_at: now + 12 * 60 * 60,
        apps: vec!["home-agent".to_string()],
    };
    crate::auth::store_session_grant(dir.path(), grant.clone()).unwrap();
    let token = issue_home_launch_token_for_auth_grant(dir.path(), "home-agent", &grant).unwrap();

    let listed = app
        .clone()
        .oneshot(post_model(token.clone(), "offers_list", json!({})))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);

    let response = app
        .oneshot(post_model(
            token,
            "runs_create",
            json!({
                "offer_id": "offer:local-text",
                "operation": "text.generate",
                "request_id": "request-home-agent-1",
                "input": { "schema": "elastos.model.input.text/v1", "prompt": "User: hi\n\nAssistant:" },
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let binding: RuntimeCreateBinding =
        serde_json::from_value(requests[1]["runtime_binding"].clone()).unwrap();
    assert_eq!(binding.capsule_id, "home-agent");
    assert_eq!(binding.principal_id, authority.principal_id);
    assert_eq!(binding.grant_id, grant.grant_id);
    drop(requests);

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let ours: Vec<_> = auth_state
        .audit
        .iter()
        .filter(|event| event.event_type.starts_with("model.run_create"))
        .collect();
    assert_eq!(ours.len(), 2);
    assert!(ours
        .iter()
        .all(|event| event.challenge_id.as_deref() == Some("request-home-agent-1")));
    assert!(ours
        .iter()
        .all(|event| event.capsule_id.as_deref() == Some("home-agent")));
}

#[tokio::test]
async fn model_run_access_injects_verified_runtime_binding() {
    let dir = tempfile::tempdir().unwrap();
    let provider = RecordingModelProvider::default();
    *provider.response.lock().await = json!({
        "status": "ok",
        "data": { "events": [] },
    });
    let app = gateway_router(model_test_state(dir.path(), provider.clone()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let grant = assistant_auth_grant(dir.path(), &authority);
    let token = issue_home_launch_token_for_auth_grant(dir.path(), "assistant", &grant).unwrap();
    let run_id = format!("run:sha256:{}", "a".repeat(64));

    let response = app
        .oneshot(post_model(
            token,
            "runs_events",
            json!({
                "run_id": run_id,
                "request_id": "request-2",
                "after_sequence": 7,
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let request = requests[0].clone();
    assert_eq!(request["op"], "runs_events");
    assert_eq!(request["after_sequence"], 7);
    let binding: RuntimeAccessBinding =
        serde_json::from_value(request["runtime_binding"].clone()).unwrap();
    assert_eq!(binding.schema, RUNTIME_ACCESS_BINDING_SCHEMA);
    assert_eq!(binding.principal_id, authority.principal_id);
    assert_eq!(binding.session_id, grant.session_id);
    assert_eq!(binding.capsule_id, "assistant");
    assert_eq!(binding.grant_id, grant.grant_id);
    assert_eq!(binding.request_id, "request-2");
    assert_eq!(binding.run_id, format!("run:sha256:{}", "a".repeat(64)));
}

#[tokio::test]
async fn caller_supplied_runtime_binding_and_legacy_authority_fields_fail_before_provider() {
    let dir = tempfile::tempdir().unwrap();
    let provider = RecordingModelProvider::default();
    let app = gateway_router(model_test_state(dir.path(), provider.clone()).await);
    let token = issue_home_launch_token(dir.path(), "assistant").unwrap();

    for body in [
        json!({
            "offer_id": "offer:flash-chat:pair-a",
            "operation": "text.generate",
            "request_id": "request-1",
            "input": {},
            "runtime_binding": {"schema": "spoofed"}
        }),
        json!({
            "offer_id": "offer:flash-chat:pair-a",
            "operation": "text.generate",
            "request_id": "request-1",
            "input": {},
            "principal_id": "spoofed"
        }),
        json!({
            "run_id": format!("run:sha256:{}", "a".repeat(64)),
            "request_id": "request-2",
            "session_id": "spoofed"
        }),
    ] {
        let op = if body.get("offer_id").is_some() {
            "runs_create"
        } else {
            "runs_get"
        };
        let response = app
            .clone()
            .oneshot(post_model(token.clone(), op, body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert!(provider.requests.lock().await.is_empty());
}

#[tokio::test]
async fn model_request_id_and_run_id_validation_fail_closed_before_provider() {
    let dir = tempfile::tempdir().unwrap();
    let provider = RecordingModelProvider::default();
    let app = gateway_router(model_test_state(dir.path(), provider.clone()).await);
    let token = issue_home_launch_token(dir.path(), "assistant").unwrap();

    for body in [
        json!({
            "offer_id": "offer:flash-chat:pair-a",
            "operation": "text.generate",
            "request_id": "  bad  ",
            "input": {}
        }),
        json!({
            "run_id": "run:sha256:ABC123",
            "request_id": "request-2"
        }),
    ] {
        let op = if body.get("offer_id").is_some() {
            "runs_create"
        } else {
            "runs_get"
        };
        let response = app
            .clone()
            .oneshot(post_model(token.clone(), op, body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert!(provider.requests.lock().await.is_empty());
}

#[tokio::test]
async fn model_create_and_cancel_audit_exact_request_ids() {
    let dir = tempfile::tempdir().unwrap();
    let provider = RecordingModelProvider::default();
    let app = gateway_router(model_test_state(dir.path(), provider.clone()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), "assistant", &authority);

    let create = app
        .clone()
        .oneshot(post_model(
            token.clone(),
            "runs_create",
            json!({
                "offer_id": "offer:flash-chat:pair-a",
                "operation": "text.generate",
                "request_id": "request-create",
                "input": {}
            }),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);

    let cancel = app
        .oneshot(post_model(
            token,
            "runs_cancel",
            json!({
                "run_id": format!("run:sha256:{}", "b".repeat(64)),
                "request_id": "request-cancel"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(cancel.status(), StatusCode::OK);

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let model_events: Vec<_> = auth_state
        .audit
        .iter()
        .filter(|event| event.event_type.starts_with("model.run_"))
        .collect();
    assert_eq!(model_events.len(), 4);
    assert_eq!(model_events[0].event_type, "model.run_create.requested");
    assert_eq!(
        model_events[0].challenge_id.as_deref(),
        Some("request-create")
    );
    assert_eq!(model_events[1].event_type, "model.run_create.completed");
    assert_eq!(
        model_events[1].challenge_id.as_deref(),
        Some("request-create")
    );
    assert_eq!(model_events[2].event_type, "model.run_cancel.requested");
    assert_eq!(
        model_events[2].challenge_id.as_deref(),
        Some("request-cancel")
    );
    assert_eq!(model_events[3].event_type, "model.run_cancel.completed");
    assert_eq!(
        model_events[3].challenge_id.as_deref(),
        Some("request-cancel")
    );
    assert!(model_events
        .iter()
        .all(|event| event.capsule_id.as_deref() == Some("assistant")));
}

#[tokio::test]
async fn model_create_error_response_audits_failed_request_id() {
    let dir = tempfile::tempdir().unwrap();
    let provider = RecordingModelProvider::default();
    *provider.response.lock().await = json!({
        "status": "error",
        "code": "selection_unavailable",
        "message": "no model offers"
    });
    let app = gateway_router(model_test_state(dir.path(), provider.clone()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), "assistant", &authority);

    let response = app
        .oneshot(post_model(
            token,
            "runs_create",
            json!({
                "offer_id": "offer:flash-chat:pair-a",
                "operation": "text.generate",
                "request_id": "request-create-error",
                "input": {}
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let model_events: Vec<_> = auth_state
        .audit
        .iter()
        .filter(|event| event.event_type.starts_with("model.run_create."))
        .collect();
    assert_eq!(model_events.len(), 2);
    assert_eq!(model_events[0].event_type, "model.run_create.requested");
    assert_eq!(
        model_events[0].challenge_id.as_deref(),
        Some("request-create-error")
    );
    assert_eq!(model_events[1].event_type, "model.run_create.failed");
    assert_eq!(
        model_events[1].challenge_id.as_deref(),
        Some("request-create-error")
    );
}

#[tokio::test]
async fn model_audit_failure_blocks_provider_invocation() {
    let dir = tempfile::tempdir().unwrap();
    let provider = RecordingModelProvider::default();
    let app = gateway_router(model_test_state(dir.path(), provider.clone()).await);
    let token = issue_home_launch_token(dir.path(), "assistant").unwrap();
    let auth_state_path = crate::auth::auth_state_path(dir.path()).unwrap();
    std::fs::create_dir_all(&auth_state_path).unwrap();

    let response = app
        .oneshot(post_model(
            token,
            "runs_create",
            json!({
                "offer_id": "offer:flash-chat:pair-a",
                "operation": "text.generate",
                "request_id": "request-audit-fail",
                "input": {}
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(provider.requests.lock().await.is_empty());
}

#[tokio::test]
async fn assistant_workspace_absent_get_is_read_only_and_returns_revision_zero() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("assistant-user"));
    let token = app_token_for_authority(dir.path(), "assistant", &authority);
    let (_, object_path) = assistant_workspace_object(dir.path(), &authority.principal_id);
    assert!(!object_path.exists());

    let response = app.oneshot(get_assistant_workspace(token)).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "revision": 0,
            "sessions": [],
            "draft": ""
        })
    );
    assert!(!object_path.exists());
}

#[tokio::test]
async fn assistant_workspace_round_trip_and_restart_preserve_exact_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("assistant-user"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let token = app_token_for_authority(dir.path(), "assistant", &authority);
    let app = gateway_router(test_state(dir.path()));
    let request = sample_workspace_put(0);

    let stored = app
        .clone()
        .oneshot(put_assistant_workspace(token.clone(), request))
        .await
        .unwrap();
    assert_eq!(stored.status(), StatusCode::OK);
    let stored_json = response_json(stored).await;
    assert_eq!(stored_json["revision"], 1);

    let restarted = gateway_router(test_state(dir.path()));
    let loaded = restarted
        .oneshot(get_assistant_workspace(token))
        .await
        .unwrap();
    assert_eq!(loaded.status(), StatusCode::OK);
    assert_eq!(response_json(loaded).await, stored_json);
    assert_eq!(stored_json["sessions"][0]["pinned"], true);
    assert_eq!(stored_json["sessions"][1]["pinned"], false);
}

#[tokio::test]
async fn assistant_workspace_content_selection_pair_round_trip_and_validation() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("assistant-user"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let token = app_token_for_authority(dir.path(), "assistant", &authority);
    let app = gateway_router(test_state(dir.path()));
    let cid = format!("bafybei{}", "a".repeat(52));
    for invalid in [
        "not-a-cid".to_string(),
        cid.to_uppercase(),
        format!("{cid} "),
        format!("bafybei{}b", "a".repeat(51)),
    ] {
        let mut request = sample_workspace_put(0);
        request["selected_model_cid"] = json!(invalid);
        let response = app
            .clone()
            .oneshot(put_assistant_workspace(token.clone(), request))
            .await
            .unwrap();
        assert!(response.status().is_client_error());
    }
    let mut partial = sample_workspace_put(0);
    partial["selected_model_cid"] = json!(cid);
    partial.as_object_mut().unwrap().remove("selected_offer_id");
    assert!(app
        .clone()
        .oneshot(put_assistant_workspace(token.clone(), partial))
        .await
        .unwrap()
        .status()
        .is_client_error());
    let mut request = sample_workspace_put(0);
    request["selected_model_cid"] = json!(cid);
    let saved = app
        .oneshot(put_assistant_workspace(token.clone(), request))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let saved = response_json(saved).await;
    assert_eq!(saved["selected_model_cid"], cid);
    assert_eq!(saved["selected_offer_id"], "offer:sample-model");
    let restarted = gateway_router(test_state(dir.path()));
    let loaded = restarted
        .oneshot(get_assistant_workspace(token))
        .await
        .unwrap();
    assert_eq!(response_json(loaded).await, saved);
}

#[tokio::test]
async fn assistant_workspace_wrong_capsule_is_forbidden_and_other_principals_stay_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("assistant-user"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let assistant_token = app_token_for_authority(dir.path(), "assistant", &authority);
    let app = gateway_router(test_state(dir.path()));

    let written = app
        .clone()
        .oneshot(put_assistant_workspace(
            assistant_token.clone(),
            sample_workspace_put(0),
        ))
        .await
        .unwrap();
    assert_eq!(written.status(), StatusCode::OK);

    let forbidden = app
        .clone()
        .oneshot(get_assistant_workspace(authority.system_token.clone()))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let other = passkey_authority_with_name_role(
        dir.path(),
        Some("guest-user"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let other_token = app_token_for_authority(dir.path(), "assistant", &other);
    let other_response = app
        .oneshot(get_assistant_workspace(other_token))
        .await
        .unwrap();
    assert_eq!(other_response.status(), StatusCode::OK);
    assert_eq!(
        response_json(other_response).await,
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "revision": 0,
            "sessions": [],
            "draft": ""
        })
    );
}

#[tokio::test]
async fn assistant_workspace_stale_and_future_revisions_fail_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("assistant-user"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let token = app_token_for_authority(dir.path(), "assistant", &authority);
    let app = gateway_router(test_state(dir.path()));

    let initial = app
        .clone()
        .oneshot(put_assistant_workspace(
            token.clone(),
            sample_workspace_put(0),
        ))
        .await
        .unwrap();
    assert_eq!(initial.status(), StatusCode::OK);

    for revision in [0_u64, 2_u64] {
        let stale = app
            .clone()
            .oneshot(put_assistant_workspace(
                token.clone(),
                sample_workspace_put(revision),
            ))
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);
    }

    let loaded = app.oneshot(get_assistant_workspace(token)).await.unwrap();
    assert_eq!(loaded.status(), StatusCode::OK);
    let loaded_json = response_json(loaded).await;
    assert_eq!(loaded_json["revision"], 1);
    assert_eq!(loaded_json["draft"], "Draft note");
}

#[tokio::test]
async fn assistant_workspace_invalid_requests_fail_closed_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("assistant-user"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let token = app_token_for_authority(dir.path(), "assistant", &authority);
    let app = gateway_router(test_state(dir.path()));
    let (_, object_path) = assistant_workspace_object(dir.path(), &authority.principal_id);

    let invalid_requests = vec![
        json!({
            "schema": "elastos.assistant.workspace/v2",
            "if_revision": 0,
            "sessions": [],
            "draft": ""
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [
                { "id": "session-1", "title": "", "mode": "chat", "messages": [] },
                { "id": "session-1", "title": "", "mode": "chat", "messages": [] }
            ],
            "draft": ""
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [{ "id": "session-1", "title": "", "mode": "invalid", "messages": [] }],
            "draft": ""
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [{
                "id": "session-1",
                "title": "",
                "mode": "chat",
                "messages": [{ "role": "invalid", "content": "hi" }]
            }],
            "draft": ""
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": (0..25).map(|index| json!({
                "id": format!("session-{index}"),
                "title": "",
                "mode": "chat",
                "messages": []
            })).collect::<Vec<_>>(),
            "draft": ""
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [{
                "id": "session-1",
                "title": "",
                "mode": "chat",
                "messages": (0..65).map(|_| json!({ "role": "user", "content": "hi" })).collect::<Vec<_>>()
            }],
            "draft": ""
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [],
            "draft": "",
            "principal_id": "spoofed"
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [],
            "draft": "",
            "authority": "spoofed"
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [],
            "draft": "",
            "session_id": "spoofed"
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [{
                "id": "s".repeat(129),
                "title": "",
                "mode": "chat",
                "messages": []
            }],
            "draft": ""
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [{
                "id": "session-1",
                "title": "t".repeat(161),
                "mode": "chat",
                "messages": []
            }],
            "draft": ""
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [],
            "draft": "",
            "path": "spoofed"
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [],
            "draft": "",
            "storage_root": "spoofed"
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [{
                "id": "session-1",
                "title": "",
                "mode": "chat",
                "messages": [{
                    "role": "assistant",
                    "content": "hi",
                    "run_id": "run:sha256:ABC123"
                }]
            }],
            "draft": ""
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [{
                "id": "session-1",
                "title": "",
                "mode": "build",
                "messages": [{
                    "role": "assistant",
                    "content": "x".repeat(8193)
                }]
            }],
            "draft": ""
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [],
            "draft": "x".repeat(16_385)
        }),
        json!({
            "schema": "elastos.assistant.workspace/v1",
            "if_revision": 0,
            "sessions": [],
            "draft": "",
            "selected_offer_id": format!("offer:{}", "x".repeat(251))
        }),
    ];

    for (index, request) in invalid_requests.into_iter().enumerate() {
        let response = app
            .clone()
            .oneshot(put_assistant_workspace(token.clone(), request))
            .await
            .unwrap();
        assert!(
            response.status().is_client_error(),
            "invalid workspace request #{index} returned {}",
            response.status()
        );
        assert!(!object_path.exists());
    }

    let oversized = app
        .oneshot(put_assistant_workspace(
            token,
            json!({
                "schema": "elastos.assistant.workspace/v1",
                "if_revision": 0,
                "sessions": [],
                "draft": "x".repeat(300 * 1024)
            }),
        ))
        .await
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(!object_path.exists());
}

#[tokio::test]
async fn assistant_workspace_is_declared_for_recovery_and_written_as_ciphertext() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("assistant-user"));
    let protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let token = app_token_for_authority(dir.path(), "assistant", &authority);
    let app = gateway_router(test_state(dir.path()));
    let (object_uri, object_path) = assistant_workspace_object(dir.path(), &authority.principal_id);
    assert!(!object_path.parent().unwrap().exists());

    let response = app
        .oneshot(put_assistant_workspace(token, sample_workspace_put(0)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let inventory = crate::api::auth_gateway::principal_root_protected_object_inventory(
        dir.path(),
        &protection.localhost_root,
    );
    assert!(inventory
        .iter()
        .map(crate::auth::PrincipalRootProtectedObjectDeclarationV1::uri)
        .any(|uri| uri.ends_with("/.AppData/ElastOS/Assistant")));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(object_path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }

    let ciphertext = std::fs::read(&object_path).unwrap();
    let raw_text = String::from_utf8_lossy(&ciphertext);
    assert!(!raw_text.contains("Draft note"));
    assert!(!raw_text.contains("elastos.assistant.workspace/v1"));

    let decrypted = crate::auth::read_principal_root_object(
        dir.path(),
        &authority.principal_id,
        &protection.localhost_root,
        &object_uri,
        &object_path,
    )
    .unwrap();
    let workspace: Value = serde_json::from_slice(&decrypted).unwrap();
    assert_eq!(workspace["revision"], 1);
    assert_eq!(workspace["selected_offer_id"], "offer:sample-model");
    assert_eq!(workspace["sessions"][0]["pinned"], true);
}

#[tokio::test]
async fn assistant_workspace_rejects_unknown_top_level_fields_in_stored_state() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("assistant-user"));
    let token = app_token_for_authority(dir.path(), "assistant", &authority);
    write_protected_workspace_fixture(
        dir.path(),
        &authority.principal_id,
        &json!({
            "schema": "elastos.assistant.workspace/v1",
            "revision": 1,
            "sessions": [],
            "draft": "",
            "unexpected": true
        }),
    );
    let app = gateway_router(test_state(dir.path()));

    let response = app.oneshot(get_assistant_workspace(token)).await.unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn assistant_workspace_revision_overflow_fails_closed_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("assistant-user"));
    let token = app_token_for_authority(dir.path(), "assistant", &authority);
    let object_path = write_protected_workspace_fixture(
        dir.path(),
        &authority.principal_id,
        &json!({
            "schema": "elastos.assistant.workspace/v1",
            "revision": u64::MAX,
            "sessions": [],
            "draft": ""
        }),
    );
    let before = std::fs::read(&object_path).unwrap();
    let app = gateway_router(test_state(dir.path()));

    let response = app
        .oneshot(put_assistant_workspace(
            token,
            json!({
                "schema": "elastos.assistant.workspace/v1",
                "if_revision": u64::MAX,
                "sessions": [],
                "draft": ""
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(std::fs::read(&object_path).unwrap(), before);
}

#[tokio::test]
async fn assistant_workspace_existing_directory_at_workspace_path_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("assistant-user"));
    let token = app_token_for_authority(dir.path(), "assistant", &authority);
    let (_, object_path) = assistant_workspace_object(dir.path(), &authority.principal_id);
    std::fs::create_dir_all(&object_path).unwrap();
    let app = gateway_router(test_state(dir.path()));

    let response = app.oneshot(get_assistant_workspace(token)).await.unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[cfg(unix)]
#[tokio::test]
async fn assistant_workspace_symlink_at_workspace_path_fails_closed() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("assistant-user"));
    let token = app_token_for_authority(dir.path(), "assistant", &authority);
    let (_, object_path) = assistant_workspace_object(dir.path(), &authority.principal_id);
    std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    symlink(dir.path().join("other-workspace.json"), &object_path).unwrap();
    let app = gateway_router(test_state(dir.path()));

    let response = app.oneshot(get_assistant_workspace(token)).await.unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn assistant_workspace_concurrent_same_revision_allows_one_write_and_one_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("assistant-user"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let token = app_token_for_authority(dir.path(), "assistant", &authority);
    let app = gateway_router(test_state(dir.path()));

    let (first, second) = tokio::join!(
        app.clone().oneshot(put_assistant_workspace(
            token.clone(),
            sample_workspace_put(0),
        )),
        app.oneshot(put_assistant_workspace(
            token.clone(),
            sample_workspace_put(0)
        )),
    );
    let statuses = [first.unwrap().status(), second.unwrap().status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );

    let loaded = gateway_router(test_state(dir.path()))
        .oneshot(get_assistant_workspace(token))
        .await
        .unwrap();
    let loaded_json = response_json(loaded).await;
    assert_eq!(loaded_json["revision"], 1);
    assert_eq!(loaded_json["sessions"][0]["pinned"], true);
}

#[test]
fn runtime_validates_typed_model_outputs_before_assistant_projection() {
    let mut valid = json!({
        "status": "ok",
        "data": {
            "events": [
                {
                    "kind": "output",
                    "data": {
                        "schema": "elastos.model.output.object/v1",
                        "uri": "elastos://object/object:studio-result"
                    }
                },
                {
                    "kind": "output",
                    "data": {
                        "schema": "elastos.model.output.content/v1",
                        "uri": "elastos://content/bafy-studio-result"
                    }
                },
                {
                    "kind": "output",
                    "data": {
                        "schema": "elastos.model.output.text/v1",
                        "text": "done"
                    }
                }
            ]
        }
    });
    project_model_provider_response("runs_events", &mut valid).unwrap();
    assert_eq!(
        valid.pointer("/data/events/0/data/resource_id"),
        Some(&json!("object:studio-result"))
    );
    assert!(valid.pointer("/data/events/0/data/uri").is_none());
    assert_eq!(
        valid.pointer("/data/events/1/data/resource_id"),
        Some(&json!("bafy-studio-result"))
    );
    assert!(valid.pointer("/data/events/1/data/uri").is_none());
    assert_eq!(
        valid.pointer("/data/events/2/data/text"),
        Some(&json!("done"))
    );

    let mut run_view = json!({
        "status": "ok",
        "data": {
            "terminal": {
                "output": {
                    "schema": "elastos.model.output.content/v1",
                    "uri": "elastos://content/run-view-result"
                }
            }
        }
    });
    project_model_provider_response("runs_get", &mut run_view).unwrap();
    assert_eq!(
        run_view.pointer("/data/terminal/output/resource_id"),
        Some(&json!("run-view-result"))
    );
    assert!(run_view.pointer("/data/terminal/output/uri").is_none());

    for output in [
        json!({
            "schema": "elastos.model.output.object/v1",
            "uri": "https://backend.example/private/result"
        }),
        json!({
            "schema": "elastos.model.output.object/v1",
            "uri": "elastos://content/wrong-namespace"
        }),
        json!({
            "schema": "elastos.model.output.content/v1",
            "uri": "elastos://content/"
        }),
        json!({
            "schema": "elastos.model.output.object/v1",
            "uri": "elastos://object/result",
            "backend_url": "https://backend.example/private/result"
        }),
        json!({
            "schema": "elastos.model.output.raw/v1",
            "bytes": "secret"
        }),
    ] {
        let mut response = json!({
            "status": "ok",
            "data": {
                "events": [{ "kind": "output", "data": output }]
            }
        });
        assert!(project_model_provider_response("runs_events", &mut response).is_err());
    }
}

#[derive(Clone)]
struct ScriptedModelProvider {
    requests: Arc<TokioMutex<Vec<Value>>>,
    replies: Arc<TokioMutex<HashMap<String, Value>>>,
    get_replies: Arc<TokioMutex<HashMap<String, Value>>>,
}

impl Default for ScriptedModelProvider {
    fn default() -> Self {
        Self {
            requests: Arc::new(TokioMutex::new(Vec::new())),
            replies: Arc::new(TokioMutex::new(HashMap::new())),
            get_replies: Arc::new(TokioMutex::new(HashMap::new())),
        }
    }
}

#[async_trait::async_trait]
impl Provider for ScriptedModelProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "resource requests are not used in this test".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["model"]
    }

    fn name(&self) -> &'static str {
        "scripted-model-provider"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        self.requests.lock().await.push(request.clone());
        if request.get("op").and_then(Value::as_str) == Some("runs_get") {
            if let Some(run_id) = request.get("run_id").and_then(Value::as_str) {
                if let Some(reply) = self.get_replies.lock().await.get(run_id).cloned() {
                    return Ok(reply);
                }
            }
        }
        let offer_id = request
            .get("offer_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if let Some(reply) = self.replies.lock().await.get(&offer_id).cloned() {
            return Ok(reply);
        }
        Ok(json!({ "status": "ok" }))
    }
}

async fn scripted_model_state(
    cache_dir: &std::path::Path,
    provider: ScriptedModelProvider,
) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("model", Arc::new(provider))
        .await
        .unwrap();
    GatewayState {
        provider_registry: Some(registry),
        collaboration_chat_product_port: None,
        collaboration_presence_product_port: None,
        carrier_endpoint: None,
        collaboration_discovery_service: None,
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

fn hosted_test_offer(
    id: &str,
    title: &str,
    processor: &str,
    api_url: &str,
    api_key: &str,
) -> Value {
    let decision = title == "Jev";
    json!({
        "id": id,
        "title": title,
        "operation": if decision { "decision.evaluate" } else { "text.generate" },
        "input_modalities": [if decision { "application/json" } else { "text/plain" }],
        "output_modalities": [if decision { "application/json" } else { "text/plain" }],
        "enabled": true,
        "adapter": {
            "kind": if decision { "open_router_decisions" } else { "open_ai_compatible_text" },
            "api_url": if decision { "https://openrouter.ai/api/alpha/decisions" } else { api_url },
            "api_key": api_key,
            "model": if decision { "typesafe/jev-1.13" } else { "fixture/model" },
            "hosted": {
                "backend_provider_label": processor,
                "selection_mode": "pinned",
                "privacy_policy_ref": "fixture:privacy:v1",
                "terms_ref": "fixture:terms:v1",
                "upstream_routing_fallback_assertion": "operator_asserted_disabled"
            }
        }
    })
}

fn decision_reply(choice: &str, risk: &str, confidence: f64) -> Value {
    json!({"status": "ok", "data": {"status": "completed", "terminal": {"output": {
        "schema": "elastos.model.output.decisions/v1", "model": "typesafe/jev-1.13",
        "answers": {
            "recommendation": {"type": "choice", "choice": choice, "confidence": confidence},
            "risk": {"type": "choice", "choice": risk}
        }
    }}}})
}

async fn status_json(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    (status, response_json(response).await)
}

fn inbox_action_request(token: String, action_id: &str) -> Request<Body> {
    test_browser_request("localhost:61180", "null")
        .method("POST")
        .uri("/api/apps/inbox/actions")
        .header("x-elastos-home-token", token)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({ "action_id": action_id })).unwrap(),
        ))
        .unwrap()
}

fn inbox_summary_request(token: String) -> Request<Body> {
    test_browser_request("localhost:61180", "null")
        .uri("/api/apps/inbox/summary")
        .header("x-elastos-home-token", token)
        .body(Body::empty())
        .unwrap()
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn owner_inbox_action_controls_hosted_route_and_system_end() {
    let dir = tempfile::tempdir().unwrap();
    crate::api::seed_model_provider_operator_offers_for_test(dir.path(), vec![]).unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let app = gateway_router(test_state(dir.path()));
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);
    let scope = crate::api::model_provider_egress_decision::EgressScope {
        offer_id: "validation:openrouter".into(),
        effect: "validate_models".into(),
        method: "GET".into(),
        url: "http://127.0.0.1:9999/models".into(),
        origin: "http://127.0.0.1:9999".into(),
        recipient: "127.0.0.1".into(),
        payer: "this Home".into(),
        provider: "OpenRouter".into(),
        purpose: "Load hosted model choices".into(),
        configuration_id: "a".repeat(64),
    };
    let id = crate::api::model_provider_egress_decision::request(
        dir.path(),
        &scope,
        Some(&authority.proof_binding_id),
    )
    .unwrap();
    let (status, summary) = status_json(
        app.clone()
            .oneshot(inbox_summary_request(inbox_token.clone()))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let entry = summary["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["action_ref"]["action_id"] == format!("model-egress-approve:{id}"))
        .unwrap();
    assert!(entry["body"]
        .as_str()
        .unwrap()
        .contains("Route: GET http://127.0.0.1:9999/models"));
    let action = format!("model-egress-approve:{id}");
    let (status, approved) = status_json(
        app.clone()
            .oneshot(inbox_action_request(inbox_token.clone(), &action))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert!(crate::api::model_provider_egress_decision::active(
        dir.path(),
        &scope,
        Some(&authority.proof_binding_id)
    )
    .is_ok());
    let get_status = || {
        test_browser_request("localhost:61180", "null")
            .uri("/api/apps/system/ai-provider")
            .header("x-elastos-home-token", authority.system_token.clone())
            .body(Body::empty())
            .unwrap()
    };
    let (status, body) = status_json(app.clone().oneshot(get_status()).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["validation_egress_approval_state"]["openrouter"],
        "approved"
    );
    let end = test_browser_request("localhost:61180", "null")
        .method("POST")
        .uri("/api/apps/system/approval-lens/revoke")
        .header("x-elastos-home-token", authority.system_token.clone())
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"id":"validation:openrouter"}).to_string(),
        ))
        .unwrap();
    let (status, ended) = status_json(app.clone().oneshot(end).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{ended}");
    assert!(crate::api::model_provider_egress_decision::active(
        dir.path(),
        &scope,
        Some(&authority.proof_binding_id)
    )
    .is_err());
    let response = app
        .oneshot(inbox_action_request(inbox_token, &action))
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn named_jev_instance_advises_hosted_http_inbox_without_auto_approve() {
    let dir = tempfile::tempdir().unwrap();
    crate::api::seed_model_provider_operator_offers_for_test(
        dir.path(),
        vec![
            hosted_test_offer(
                "model:openrouter",
                "Jev",
                "OpenRouter",
                "https://openrouter.ai/api/v1/chat/completions",
                "sk-or-fixture-secret",
            ),
            hosted_test_offer(
                "model:venice",
                "Venice",
                "Venice",
                "https://api.venice.ai/api/v1/chat/completions",
                "sk-vnz-fixture-secret",
            ),
        ],
    )
    .unwrap();
    let provider = ScriptedModelProvider::default();
    provider.replies.lock().await.insert(
        "model:openrouter".to_string(),
        decision_reply("approve", "low", 0.8),
    );
    let app = gateway_router(scripted_model_state(dir.path(), provider.clone()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let grant = assistant_auth_grant(dir.path(), &authority);
    let assistant_token =
        issue_home_launch_token_for_auth_grant(dir.path(), "assistant", &grant).unwrap();
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);

    let (status, body) = status_json(
        app.clone()
            .oneshot(post_model(
                assistant_token.clone(),
                "runs_create",
                json!({
                    "offer_id": "model:venice",
                    "operation": "text.generate",
                    "request_id": "request-venice-1",
                    "input": { "messages": [{ "role": "user", "content": "hello" }] }
                }),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "error");
    assert_eq!(body["code"], "approval_required");
    let requests = provider.requests.lock().await.clone();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0]["offer_id"], "model:openrouter");
    let eval_request_id = requests[0]["runtime_binding"]["request_id"]
        .as_str()
        .unwrap();
    assert!(
        eval_request_id.starts_with("jev-eval:hosted-http:model_venice:"),
        "{eval_request_id}"
    );
    let eval_prompt = requests[0]["input"].to_string();
    assert_eq!(requests[0]["operation"], "decision.evaluate");
    assert_eq!(
        requests[0]["input"]["schema"],
        "elastos.model.input.decisions/v1"
    );
    assert!(eval_prompt.contains("Venice"));
    assert!(!eval_prompt.contains("sk-or-fixture-secret"));
    assert!(!eval_prompt.contains("sk-vnz-fixture-secret"));
    assert!(!eval_prompt.contains("api_key"));
    assert!(!eval_prompt.contains("openrouter.ai"));
    assert!(!eval_prompt.contains("hello"));

    let (status, summary) = status_json(
        app.clone()
            .oneshot(inbox_summary_request(inbox_token.clone()))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    let entry = summary["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "external_http_request")
        .unwrap();
    let body_text = entry["body"].as_str().unwrap();
    assert!(body_text.contains("Requested by: Assistant"));
    assert!(body_text.contains("Connection: Venice"));
    assert!(body_text.contains("Prompt recipient: Venice"));
    assert!(body_text.contains("Payer: this Home"));
    assert!(body_text.contains("until you end approval in System > Models"));
    assert!(body_text.contains("This pending prompt stays in Assistant"));
    assert!(body_text.contains("Jev advice: approve · risk: low · reported confidence: 80%"));
    assert!(body_text.contains("Advice reason: Runtime rubric:"));
    assert!(!body_text.contains("sk-or-fixture-secret"));
    assert!(!body_text.contains("sk-vnz-fixture-secret"));
    let action_id = entry["action_ref"]["action_id"].as_str().unwrap();
    assert!(action_id.starts_with("hosted-http-approve:"));

    let (status, acted) = status_json(
        app.clone()
            .oneshot(inbox_action_request(inbox_token.clone(), action_id))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{acted}");

    let request_id = crate::jev_approval_lens::hosted_http_request_id("model:venice").unwrap();
    let record = crate::jev_approval_lens::load_record(dir.path(), &request_id).unwrap();
    assert_eq!(record.recommendation.recommendation, "approve");
    assert_eq!(record.human_decision.as_deref(), Some("approve"));
    assert!(record.actual_outcome.is_none());
    assert!(record
        .policy
        .blocked_choices
        .iter()
        .any(|choice| choice == "auto_approve"));

    let (status, retry) = status_json(
        app.clone()
            .oneshot(post_model(
                assistant_token.clone(),
                "runs_create",
                json!({
                    "offer_id": "model:venice",
                    "operation": "text.generate",
                    "request_id": "request-venice-2",
                    "input": { "messages": [{ "role": "user", "content": "hello" }] }
                }),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{retry}");
    assert_eq!(retry["status"], "ok");
    let requests = provider.requests.lock().await.clone();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_eq!(requests[1]["offer_id"], "model:venice");
    let record = crate::jev_approval_lens::load_record(dir.path(), &request_id).unwrap();
    assert_eq!(record.human_decision.as_deref(), Some("approve"));
    assert_eq!(record.actual_outcome.as_deref(), Some("accepted"));
    let encoded = serde_json::to_string(&record).unwrap();
    assert!(!encoded.contains("sk-or-fixture-secret"));
    assert!(!encoded.contains("sk-vnz-fixture-secret"));

    let (status, connection_status) = status_json(
        app.clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .method("GET")
                    .uri("/api/apps/system/ai-provider")
                    .header("x-elastos-home-token", authority.system_token.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{connection_status}");
    let approved = connection_status["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|connection| connection["id"] == "model:venice")
        .unwrap();
    assert_eq!(approved["approval_state"], "approved");

    let revoke = |token: String| {
        test_browser_request("localhost:61180", "null")
            .method("POST")
            .uri("/api/apps/system/approval-lens/revoke")
            .header("x-elastos-home-token", token)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({ "id": "model:venice" })).unwrap(),
            ))
            .unwrap()
    };
    let guest = passkey_authority_with_name_role(
        dir.path(),
        Some("guest"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let response = app
        .clone()
        .oneshot(revoke(guest.system_token))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let (status, ended) = status_json(
        app.clone()
            .oneshot(revoke(authority.system_token.clone()))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ended}");
    assert_eq!(ended["approval"], "ended");
    assert!(!crate::jev_approval_lens::approved_connection(
        dir.path(),
        "model:venice"
    ));
    let record = crate::jev_approval_lens::load_record(dir.path(), &request_id).unwrap();
    assert_eq!(record.human_decision.as_deref(), Some("defer"));

    let (status, after_revoke) = status_json(
        app.clone()
            .oneshot(post_model(
                assistant_token,
                "runs_create",
                json!({
                    "offer_id": "model:venice", "operation": "text.generate",
                    "request_id": "request-venice-3",
                    "input": { "messages": [{ "role": "user", "content": "after revoke" }] }
                }),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after_revoke}");
    assert_eq!(after_revoke["code"], "approval_required");
    assert_eq!(provider.requests.lock().await.len(), 2);
    let (status, summary) = status_json(
        app.oneshot(inbox_summary_request(inbox_token))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    let pending = summary["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| {
            entry["kind"] == "external_http_request"
                && entry["action_ref"]["action_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("hosted-http-approve:"))
        })
        .expect("ending approval creates a new Inbox decision");
    assert!(pending["body"]
        .as_str()
        .unwrap()
        .contains("This pending prompt stays in Assistant"));
}

#[tokio::test]
async fn named_jev_waits_for_run_terminal_before_inbox_advice() {
    let dir = tempfile::tempdir().unwrap();
    crate::api::seed_model_provider_operator_offers_for_test(
        dir.path(),
        vec![
            hosted_test_offer(
                "model:openrouter",
                "Jev",
                "OpenRouter",
                "https://openrouter.ai/api/v1/chat/completions",
                "sk-or-fixture-secret",
            ),
            hosted_test_offer(
                "model:venice",
                "Venice",
                "Venice",
                "https://api.venice.ai/api/v1/chat/completions",
                "sk-vnz-fixture-secret",
            ),
        ],
    )
    .unwrap();
    let run_id = format!("run:sha256:{}", "ab".repeat(32));
    let provider = ScriptedModelProvider::default();
    provider.replies.lock().await.insert(
        "model:openrouter".to_string(),
        json!({ "status": "ok", "data": { "run_id": run_id } }),
    );
    provider
        .get_replies
        .lock()
        .await
        .insert(run_id.clone(), decision_reply("approve", "low", 0.7));
    let app = gateway_router(scripted_model_state(dir.path(), provider.clone()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let grant = assistant_auth_grant(dir.path(), &authority);
    let assistant_token =
        issue_home_launch_token_for_auth_grant(dir.path(), "assistant", &grant).unwrap();
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);

    let (status, body) = status_json(
        app.clone()
            .oneshot(post_model(
                assistant_token,
                "runs_create",
                json!({
                    "offer_id": "model:venice",
                    "operation": "text.generate",
                    "request_id": "request-venice-terminal",
                    "input": { "messages": [{ "role": "user", "content": "hello" }] }
                }),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "approval_required");
    let requests = provider.requests.lock().await.clone();
    assert!(
        requests
            .iter()
            .any(|request| request["op"] == "runs_create"
                && request["offer_id"] == "model:openrouter"),
        "{requests:?}"
    );
    assert!(
        requests
            .iter()
            .any(|request| request["op"] == "runs_get" && request["run_id"] == run_id),
        "{requests:?}"
    );
    let (status, summary) = status_json(
        app.oneshot(inbox_summary_request(inbox_token))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    let entry = summary["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "external_http_request")
        .unwrap();
    let body_text = entry["body"].as_str().unwrap();
    assert!(body_text.contains("Jev advice: approve"));
    assert!(body_text.contains("Runtime rubric:"));
}

#[tokio::test]
async fn named_jev_malformed_or_failed_reply_still_opens_inbox() {
    let dir = tempfile::tempdir().unwrap();
    crate::api::seed_model_provider_operator_offers_for_test(
        dir.path(),
        vec![
            hosted_test_offer(
                "model:openrouter",
                "Jev",
                "OpenRouter",
                "https://openrouter.ai/api/v1/chat/completions",
                "sk-or-fixture-secret",
            ),
            hosted_test_offer(
                "model:venice",
                "Venice",
                "Venice",
                "https://api.venice.ai/api/v1/chat/completions",
                "sk-vnz-fixture-secret",
            ),
        ],
    )
    .unwrap();
    let provider = ScriptedModelProvider::default();
    provider.replies.lock().await.insert(
        "model:openrouter".to_string(),
        decision_reply("auto_approve", "low", 0.8),
    );
    let app = gateway_router(scripted_model_state(dir.path(), provider.clone()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let grant = assistant_auth_grant(dir.path(), &authority);
    let assistant_token =
        issue_home_launch_token_for_auth_grant(dir.path(), "assistant", &grant).unwrap();
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);

    let (status, body) = status_json(
        app.clone()
            .oneshot(post_model(
                assistant_token,
                "runs_create",
                json!({
                    "offer_id": "model:venice",
                    "operation": "text.generate",
                    "request_id": "request-venice-bad",
                    "input": { "messages": [{ "role": "user", "content": "hello" }] }
                }),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "approval_required");
    let (status, summary) = status_json(
        app.oneshot(inbox_summary_request(inbox_token))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    let body_text = summary["notifications"]["entries"][0]["body"]
        .as_str()
        .unwrap();
    assert!(body_text.contains("Jev advice: unavailable"));
    assert!(body_text.contains("risk: unknown"));
    assert!(body_text.contains("reported confidence: not reported"));
    let request_id = crate::jev_approval_lens::hosted_http_request_id("model:venice").unwrap();
    let record = crate::jev_approval_lens::load_record(dir.path(), &request_id).unwrap();
    assert_eq!(record.recommendation.recommendation, "unavailable");
    assert!(record.recommendation.needs_human_review);
}

#[tokio::test]
async fn named_jev_provider_failure_still_opens_inbox() {
    let dir = tempfile::tempdir().unwrap();
    crate::api::seed_model_provider_operator_offers_for_test(
        dir.path(),
        vec![
            hosted_test_offer(
                "model:openrouter",
                "Jev",
                "OpenRouter",
                "https://openrouter.ai/api/v1/chat/completions",
                "sk-or-fixture-secret",
            ),
            hosted_test_offer(
                "model:venice",
                "Venice",
                "Venice",
                "https://api.venice.ai/api/v1/chat/completions",
                "sk-vnz-fixture-secret",
            ),
        ],
    )
    .unwrap();
    let provider = ScriptedModelProvider::default();
    provider.replies.lock().await.insert(
        "model:openrouter".to_string(),
        json!({ "status": "error", "code": "provider_error", "message": "upstream timeout" }),
    );
    let app = gateway_router(scripted_model_state(dir.path(), provider).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let grant = assistant_auth_grant(dir.path(), &authority);
    let assistant_token =
        issue_home_launch_token_for_auth_grant(dir.path(), "assistant", &grant).unwrap();
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);

    let (status, body) = status_json(
        app.clone()
            .oneshot(post_model(
                assistant_token,
                "runs_create",
                json!({
                    "offer_id": "model:venice",
                    "operation": "text.generate",
                    "request_id": "request-venice-fail",
                    "input": { "messages": [{ "role": "user", "content": "hello" }] }
                }),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "approval_required");
    let (status, summary) = status_json(
        app.oneshot(inbox_summary_request(inbox_token))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    let body_text = summary["notifications"]["entries"][0]["body"]
        .as_str()
        .unwrap();
    assert!(body_text.contains("Jev advice: unavailable"));
    assert!(body_text.contains("You decide."));
}

#[tokio::test]
async fn named_jev_retries_eval_after_unsigned_record_cleared() {
    let dir = tempfile::tempdir().unwrap();
    crate::api::seed_model_provider_operator_offers_for_test(
        dir.path(),
        vec![
            hosted_test_offer(
                "model:openrouter",
                "Jev",
                "OpenRouter",
                "https://openrouter.ai/api/v1/chat/completions",
                "sk-or-fixture-secret",
            ),
            hosted_test_offer(
                "model:venice",
                "Venice",
                "Venice",
                "https://api.venice.ai/api/v1/chat/completions",
                "sk-vnz-fixture-secret",
            ),
        ],
    )
    .unwrap();
    let provider = ScriptedModelProvider::default();
    provider.replies.lock().await.insert(
        "model:openrouter".to_string(),
        decision_reply("approve", "low", 0.8),
    );
    let app = gateway_router(scripted_model_state(dir.path(), provider.clone()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let grant = assistant_auth_grant(dir.path(), &authority);
    let assistant_token =
        issue_home_launch_token_for_auth_grant(dir.path(), "assistant", &grant).unwrap();
    let create = json!({
        "offer_id": "model:venice",
        "operation": "text.generate",
        "request_id": "request-venice-retry",
        "input": { "messages": [{ "role": "user", "content": "hello" }] }
    });
    let (status, body) = status_json(
        app.clone()
            .oneshot(post_model(
                assistant_token.clone(),
                "runs_create",
                create.clone(),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "approval_required");
    let request_id = crate::jev_approval_lens::hosted_http_request_id("model:venice").unwrap();
    let record_path = dir
        .path()
        .join("jev-approval-lens")
        .join(format!("{}.json", request_id.replace(':', "_")));
    std::fs::remove_file(&record_path).unwrap();
    let (status, retry) = status_json(
        app.oneshot(post_model(assistant_token, "runs_create", create))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{retry}");
    assert_eq!(retry["code"], "approval_required");
    let requests = provider.requests.lock().await.clone();
    let eval_ids: Vec<&str> = requests
        .iter()
        .filter(|request| request["offer_id"] == "model:openrouter")
        .map(|request| request["runtime_binding"]["request_id"].as_str().unwrap())
        .collect();
    assert_eq!(eval_ids.len(), 2, "{requests:?}");
    assert_ne!(eval_ids[0], eval_ids[1], "{eval_ids:?}");
}

#[tokio::test]
async fn hosted_configuration_rejection_is_reported_after_save_and_remove() {
    let dir = tempfile::tempdir().unwrap();
    let provider = RecordingModelProvider::default();
    *provider.response.lock().await = json!({
        "status": "error", "code": "invalid_request", "message": "private provider detail"
    });
    let state = model_test_state(dir.path(), provider.clone()).await;
    let id = "model:hosted-0123456789abcdef0123456789abcdef";
    let error = crate::api::model_provider_config::save_hosted_offer(
        dir.path(),
        state.provider_registry.as_deref(),
        crate::api::model_provider_config::HostedOfferSave {
            provider: crate::api::model_provider_config::HostedAiProvider::OpenRouter,
            api_key: "fixture-key",
            model: "fixture/model",
            expected_response_model: None,
            privacy: None,
            name: "Fixture",
            instance_id: Some(id),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "model activation pending");
    assert_eq!(
        crate::api::ai_provider_status(dir.path())
            .unwrap()
            .connections[0]
            .id,
        id
    );
    let error = crate::api::remove_hosted_offer(dir.path(), state.provider_registry.as_deref(), id)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "model retirement pending");
    assert!(crate::api::ai_provider_status(dir.path())
        .unwrap()
        .connections
        .is_empty());
    let calls = provider.requests.lock().await;
    assert_eq!(calls.iter().filter(|call| call["op"] == "init").count(), 2);
}
