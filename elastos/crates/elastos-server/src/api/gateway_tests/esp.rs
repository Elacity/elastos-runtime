use super::*;

const ESP_INVOKE_CAPSULE: &str = "esp-invoke-test";
const ESP_INVOKE_INTERFACE: &str = "elastos.test.invoke";

#[derive(Default)]
struct EspInvokeProvider {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl Provider for EspInvokeProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "ESP invoke test provider supports only raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["elastos"]
    }

    fn name(&self) -> &'static str {
        "esp-invoke-provider"
    }

    async fn send_raw(
        &self,
        _request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(json!({ "status": "ok", "data": { "dispatched": true } }))
    }
}

fn write_esp_invoke_capsule(data_dir: &std::path::Path) {
    write_test_browser_capsule(
        data_dir,
        ESP_INVOKE_CAPSULE,
        "app",
        "ESP invocation contract test",
        None,
    );
    let manifest = json!({
        "schema": "elastos.capsule/v1",
        "name": ESP_INVOKE_CAPSULE,
        "version": "0.1.0",
        "description": "ESP invocation contract test",
        "author": "elastos",
        "role": "app",
        "type": "wasm",
        "runtime_abi": "elastos.runtime-projection/v1",
        "bus_contract": "elastos.runtime-projection/v1",
        "execution": "web-projection",
        "projections": ["web", "affordances"],
        "entrypoint": "browser/index.html",
        "interfaces": [{
            "id": ESP_INVOKE_INTERFACE,
            "version": "0.1.0",
            "methods": [
                {
                    "id": "runtime.catalog",
                    "risk": "read",
                    "approval": "runtime_policy",
                    "audit": "event",
                    "resource": "elastos://capsules/*",
                    "operation": "list"
                },
                {
                    "id": "runtime.launch",
                    "risk": "launch",
                    "approval": "runtime_policy",
                    "audit": "event",
                    "resource": "elastos://capsules/*",
                    "operation": "launch"
                },
                {
                    "id": "provider.status",
                    "risk": "read",
                    "approval": "runtime_policy",
                    "audit": "event",
                    "resource": "elastos://chain/*",
                    "operation": "status"
                },
                {
                    "id": "provider.unbound",
                    "risk": "read",
                    "approval": "runtime_policy",
                    "audit": "event",
                    "resource": "elastos://chain/*",
                    "operation": "unknown_operation"
                },
                {
                    "id": "runtime.approval-required",
                    "risk": "privileged",
                    "approval": "user",
                    "audit": "full",
                    "resource": "elastos://capsules/*",
                    "operation": "list"
                }
            ]
        }]
    });
    std::fs::write(
        data_dir
            .join("capsules")
            .join(ESP_INVOKE_CAPSULE)
            .join("capsule.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

async fn invoke_esp_method(
    app: &axum::Router,
    token: &str,
    method: &str,
) -> (StatusCode, serde_json::Value) {
    invoke_esp_method_with_input(app, token, method, json!({})).await
}

async fn invoke_esp_method_with_input(
    app: &axum::Router,
    token: &str,
    method: &str,
    input: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/capsules/interfaces/invoke")
                .header(CONTENT_TYPE, "application/json")
                .header("x-elastos-home-token", token)
                .body(Body::from(
                    json!({
                        "request_id": format!("esp-invoke-{method}"),
                        "capsule": ESP_INVOKE_CAPSULE,
                        "interface": ESP_INVOKE_INTERFACE,
                        "method": method,
                        "input": input
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn esp_initialize_describes_existing_projection_routes() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/esp/initialize")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["schema"], "elastos.esp.initialize/v0");
    assert_eq!(payload["protocol"], "elastos-shell-protocol");
    assert_eq!(payload["esp_version"], "0");
    assert_eq!(payload["transport"], "http-json");
    assert_eq!(payload["transport_scope"], "local_runtime_adapter");
    assert!(payload["supported_schemas"]
        .as_array()
        .unwrap()
        .iter()
        .any(|schema| schema == "elastos.inspect.gate-preview/v1"));
    assert!(payload["facts"].as_array().unwrap().iter().any(|fact| {
        fact["schema"] == "elastos.capsules.catalog/v1"
            && fact["operation"] == "capsules.catalog"
            && fact["route"] == "/api/capsules/catalog"
            && fact["authority"]
                .as_str()
                .unwrap()
                .contains("descriptors are not grants")
    }));
    assert!(payload["facts"].as_array().unwrap().iter().any(|fact| {
        fact["schema"] == "elastos.inspect.gate-preview/v1"
            && fact["route"] == "/api/provider/inspect/plan"
            && fact["authority"].as_str().unwrap().contains("Preview-only")
    }));
    assert!(payload["verbs"].as_array().unwrap().iter().any(|verb| {
        verb["name"] == "inbox.approve_inspect_action"
            && verb["auth"]
                .as_str()
                .unwrap()
                .contains("fresh same-principal passkey")
    }));
    assert!(payload["verbs"].as_array().unwrap().iter().any(|verb| {
        verb["name"] == "capsule.invoke_runtime_policy_affordance"
            && verb["effect"]
                .as_str()
                .unwrap()
                .contains("exact request ID, principal, capsule, interface, method, resource, and body binding")
            && verb["gate"]
                == "Provider-path-only, unbound, unknown, and approval-required operations fail closed."
    }));
    assert!(payload["invariants"]
        .as_array()
        .unwrap()
        .iter()
        .any(|invariant| {
            invariant
                .as_str()
                .unwrap()
                .contains("not an authority layer")
        }));
    for invariant in [
        "Verification proves evidence only; it does not authorize or make a method executable.",
        "Declared risk is advisory metadata; Runtime bindings and route policy decide executability and authority.",
        "Missing trust, permission, binding, or policy evidence is unknown, never safe.",
        "Routes, frames, iframe placement, and HTTP success are transport or presentation facts, not authority.",
        "Effect completion requires an exact request binding and matching Runtime result receipt.",
    ] {
        assert!(payload["invariants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|served| served == invariant));
    }

    let inspect_without_system_token = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/inspect/plan")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"id":"capsule:exit-provider","operation":"status"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(inspect_without_system_token.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn esp_initialize_keeps_http_adapter_separate_from_authority_model() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/esp/initialize")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let descriptor: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(descriptor["protocol"], "elastos-shell-protocol");
    assert_eq!(descriptor["transport"], "http-json");
    assert_eq!(descriptor["transport_scope"], "local_runtime_adapter");

    let facts = descriptor["facts"].as_array().unwrap();
    assert!(facts.iter().all(|fact| {
        let operation = fact["operation"].as_str().unwrap();
        let route = fact["route"].as_str().unwrap();
        !operation.is_empty()
            && operation != route
            && !operation.starts_with("/api/")
            && route.starts_with("/api/")
    }));
    assert!(descriptor["invariants"]
        .as_array()
        .unwrap()
        .iter()
        .any(|invariant| invariant
            .as_str()
            .unwrap()
            .contains("HTTP method and route fields describe the current local adapter")));
    assert!(descriptor["invariants"]
        .as_array()
        .unwrap()
        .iter()
        .any(|invariant| invariant
            .as_str()
            .unwrap()
            .contains("Future Carrier transport may expose the same schemas only by preserving the same Runtime gates")));

    let negotiated = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/esp/initialize")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"esp_version":"0","accepts":["elastos.capsules.catalog/v1"]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(negotiated.status(), StatusCode::OK);
    let body = axum::body::to_bytes(negotiated.into_body(), usize::MAX)
        .await
        .unwrap();
    let negotiated: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(negotiated["facts"], descriptor["facts"]);
    assert_eq!(negotiated["verbs"], descriptor["verbs"]);
    assert_eq!(negotiated["invariants"], descriptor["invariants"]);
    assert_eq!(
        negotiated["accepted"],
        json!(["elastos.capsules.catalog/v1"])
    );
}

#[tokio::test]
async fn esp_initialize_negotiates_schema_tags_without_authority() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/esp/initialize")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"esp_version":"0","accepts":["elastos.inspect.gate-preview/v1","elastos.unknown/v1"]}"#,
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
    assert_eq!(
        payload["accepted"],
        json!(["elastos.inspect.gate-preview/v1"])
    );
    assert_eq!(payload["unsupported"], json!(["elastos.unknown/v1"]));
    assert_eq!(
        payload["verbs"][0]["route"],
        "/api/provider/inspect/request_act"
    );

    let unsupported_version = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/esp/initialize")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"esp_version":"1","accepts":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unsupported_version.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(unsupported_version.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["code"], "unsupported_esp_version");
    assert_eq!(payload["supported"], json!(["0"]));
}

#[tokio::test]
async fn esp_generic_invocation_executes_only_explicit_runtime_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = test_state(dir.path());
    write_esp_invoke_capsule(dir.path());

    let provider = Arc::new(EspInvokeProvider::default());
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("chain", provider.clone())
        .await
        .unwrap();
    state.provider_registry = Some(registry);
    let app = gateway_router(state);

    let system_token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();
    let interfaces_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/capsules/interfaces")
                .header("x-elastos-home-token", system_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(interfaces_response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(interfaces_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let interfaces: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let bindings = interfaces["interfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["capsule"] == ESP_INVOKE_CAPSULE)
        .and_then(|entry| entry["bindings"].as_array())
        .expect("ESP invoke fixture bindings");
    let binding = |method: &str| {
        bindings
            .iter()
            .find(|binding| binding["method"] == method)
            .unwrap()
    };
    assert_eq!(binding("runtime.catalog")["state"], "executable");
    assert_eq!(binding("runtime.catalog")["handler_kind"], "runtime");
    assert_eq!(binding("runtime.catalog")["executable"], true);
    assert_eq!(binding("runtime.launch")["state"], "executable");
    assert_eq!(binding("runtime.launch")["handler_kind"], "runtime");
    assert_eq!(binding("runtime.launch")["executable"], true);
    assert_eq!(binding("provider.status")["state"], "provider-path-only");
    assert_eq!(binding("provider.status")["executable"], false);
    assert_eq!(binding("provider.unbound")["state"], "unbound");
    assert_eq!(binding("provider.unbound")["executable"], false);
    assert_eq!(
        binding("runtime.approval-required")["state"],
        "approval-required"
    );
    assert_eq!(binding("runtime.approval-required")["executable"], false);

    let authority = passkey_authority_with_name(dir.path(), Some("esp-invoker"));
    let token = app_token_for_authority(dir.path(), ESP_INVOKE_CAPSULE, &authority);
    let (status, body) = invoke_esp_method(&app, &token, "runtime.catalog").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["request_id"], "esp-invoke-runtime.catalog");
    assert_eq!(
        body["request_binding"]["schema"],
        "elastos.esp.request-binding/v1"
    );
    assert_eq!(body["request_binding"]["request_id"], body["request_id"]);
    assert_eq!(body["request_binding"]["capsule"], ESP_INVOKE_CAPSULE);
    assert_eq!(body["request_binding"]["principal"], authority.principal_id);
    assert_eq!(body["request_binding"]["interface"], ESP_INVOKE_INTERFACE);
    assert_eq!(body["request_binding"]["method"], "runtime.catalog");
    assert_eq!(
        body["request_binding"]["resources"],
        json!(["elastos://capsules/*"])
    );
    assert_eq!(body["request_binding"]["preview"], json!({}));
    assert!(body["output"]["catalog"].is_object());

    let (status, launch) = invoke_esp_method_with_input(
        &app,
        &token,
        "runtime.launch",
        json!({ "target": ESP_INVOKE_CAPSULE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(launch["request_id"], "esp-invoke-runtime.launch");
    assert_eq!(launch["request_binding"]["method"], "runtime.launch");
    assert_eq!(
        launch["request_binding"]["resources"],
        json!(["elastos://capsules/*"])
    );
    assert_eq!(
        launch["request_binding"]["principal"],
        authority.principal_id
    );
    assert_eq!(
        launch["request_binding"]["sha256"].as_str().unwrap().len(),
        64
    );
    assert_eq!(
        launch["request_binding"]["preview"],
        json!({ "target": ESP_INVOKE_CAPSULE })
    );
    assert_eq!(launch["output"]["target"], ESP_INVOKE_CAPSULE);

    for method in ["provider.status", "provider.unbound"] {
        let (status, body) = invoke_esp_method(&app, &token, method).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(body["status"], "error");
        assert_eq!(body["code"], "affordance_not_bound");
    }

    let (status, body) = invoke_esp_method(&app, &token, "runtime.approval-required").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["status"], "error");
    assert_eq!(body["code"], "approval_required");

    let (status, body) = invoke_esp_method(&app, &token, "unknown.method").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["schema"], "elastos.capsules.invoke-result/v1");
    assert_eq!(body["status"], "error");
    assert_eq!(body["code"], "affordance_not_declared");
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}
