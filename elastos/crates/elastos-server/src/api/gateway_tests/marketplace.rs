use super::*;

#[tokio::test]
async fn test_marketplace_catalog_route_is_registered_and_auth_gated() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let denied = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/capsules/catalog")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let token = issue_home_launch_token(dir.path(), MARKETPLACE_CAPSULE_ID).unwrap();
    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/capsules/catalog")
                .header("x-elastos-home-token", token.clone())
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
    assert_eq!(payload["schema"], "elastos.capsules.catalog/v1");
    assert!(
        payload["capsules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|capsule| capsule["name"] != "bus-v1-conformance"
                && capsule["name"] != "marketplace")
    );

    let marketplace_scoped_response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/marketplace/catalog")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(marketplace_scoped_response.status(), StatusCode::OK);
}

#[tokio::test]
async fn model_consumers_can_read_catalog_with_current_scoped_authority() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority(dir.path());
    for consumer in ["home-agent", "assistant"] {
        let token = app_token_for_authority(dir.path(), consumer, &authority);
        let request = test_browser_request("localhost:61180", "null")
            .header("x-elastos-home-token", token.as_str())
            .body(Body::empty())
            .unwrap();
        let context = super::super::gateway_home_token::require_home_launch_token_context(
            dir.path(),
            request.headers(),
            consumer,
        )
        .unwrap();
        let expired =
            super::super::gateway_home_token::issue_expired_home_launch_token_with_context(
                dir.path(),
                consumer,
                &context,
            )
            .unwrap();
        let unrelated = app_token_for_authority(dir.path(), "documents", &authority);
        for route in [
            "/api/capsules/catalog",
            "/api/capsules/interfaces",
            "/api/apps/marketplace/catalog",
        ] {
            for (candidate, origin, expected) in [
                (Some(token.as_str()), "null", StatusCode::OK),
                (
                    Some(token.as_str()),
                    "https://unrelated.example",
                    StatusCode::FORBIDDEN,
                ),
                (Some(expired.as_str()), "null", StatusCode::FORBIDDEN),
                (Some(unrelated.as_str()), "null", StatusCode::FORBIDDEN),
                (None, "null", StatusCode::FORBIDDEN),
            ] {
                let mut request = test_browser_request("localhost:61180", origin).uri(route);
                if let Some(token) = candidate {
                    request = request.header("x-elastos-home-token", token);
                }
                let response = app
                    .clone()
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                assert_eq!(response.status(), expected, "{consumer} {route} {origin}");
            }
        }
    }
}
