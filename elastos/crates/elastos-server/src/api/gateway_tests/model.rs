use super::*;

struct MockModelProvider;

#[async_trait::async_trait]
impl Provider for MockModelProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock model provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["elastos"]
    }

    fn name(&self) -> &'static str {
        "mock-model-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match request.get("op").and_then(|value| value.as_str()) {
            Some("ping") => Ok(json!({
                "status": "ok",
                "data": { "provider": "model-provider", "version": "0.1.0-dev" }
            })),
            _ => Err(ProviderError::Provider("unsupported op".into())),
        }
    }
}

async fn model_test_state(cache_dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("model", Arc::new(MockModelProvider))
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

#[tokio::test]
async fn home_gui_token_can_ping_model_provider() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(model_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), HOME_GUI_SHELL_ID).unwrap();

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/model/ping")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["status"], "ok");
    assert_eq!(payload["data"]["provider"], "model-provider");
}

#[tokio::test]
async fn model_provider_rejects_unknown_op() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(model_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), HOME_GUI_SHELL_ID).unwrap();

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/model/runs_destroy")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn model_provider_rejects_wrong_app_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(model_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/model/ping")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn model_provider_requires_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(model_test_state(dir.path()).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/model/ping")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_client_error());
}
