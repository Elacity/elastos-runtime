use super::*;
use crate::api::gateway::gateway_home_token;
use axum::body::Body;
use elastos_model_contract::{
    model_input_hash, RuntimeAccessBinding, RuntimeCreateBinding, RUNTIME_ACCESS_BINDING_SCHEMA,
    RUNTIME_CREATE_BINDING_SCHEMA,
};
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
async fn assistant_cannot_invoke_unsupported_model_operation() {
    let dir = tempfile::tempdir().unwrap();
    let provider = RecordingModelProvider::default();
    let app = gateway_router(model_test_state(dir.path(), provider.clone()).await);
    let token = issue_home_launch_token(dir.path(), "assistant").unwrap();

    let response = app
        .oneshot(post_model(token, "offer_get", json!({})))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
async fn model_run_access_injects_verified_runtime_binding() {
    let dir = tempfile::tempdir().unwrap();
    let provider = RecordingModelProvider::default();
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
}
