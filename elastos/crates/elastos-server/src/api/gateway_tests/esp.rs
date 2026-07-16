use super::*;

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
