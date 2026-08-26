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
