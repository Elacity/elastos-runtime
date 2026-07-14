use super::*;

#[tokio::test]
async fn test_marketplace_catalog_route_is_registered_and_auth_gated() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
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
            Request::builder()
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
            Request::builder()
                .uri("/api/apps/marketplace/catalog")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(marketplace_scoped_response.status(), StatusCode::OK);
}
