use super::super::*;

#[tokio::test]
async fn test_system_token_can_read_chain_provider_status() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(chain_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();

    let networks = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/chain/networks")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(networks.status(), StatusCode::OK);
    let networks_body = axum::body::to_bytes(networks.into_body(), usize::MAX)
        .await
        .unwrap();
    let networks_payload: serde_json::Value = serde_json::from_slice(&networks_body).unwrap();
    assert_eq!(networks_payload["status"], "ok");
    assert_eq!(networks_payload["data"]["networks"][0]["id"], "esc-mainnet");
    assert!(networks_payload["data"]["networks"][0]
        .get("rpc_url")
        .is_none());

    let status = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/chain/status")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"network":"esc-mainnet"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    let status_body = axum::body::to_bytes(status.into_body(), usize::MAX)
        .await
        .unwrap();
    let status_payload: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
    assert_eq!(status_payload["status"], "ok");
    assert_eq!(status_payload["data"]["block_number"], 42);

    let sync_health = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/chain/sync_health")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"network":"esc-mainnet"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sync_health.status(), StatusCode::OK);
    let sync_health_body = axum::body::to_bytes(sync_health.into_body(), usize::MAX)
        .await
        .unwrap();
    let sync_health_payload: serde_json::Value = serde_json::from_slice(&sync_health_body).unwrap();
    assert_eq!(sync_health_payload["status"], "ok");
    assert_eq!(sync_health_payload["data"]["healthy"], true);
    assert!(sync_health_payload["data"].get("rpc_url").is_none());

    let lifecycle = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/chain/node_lifecycle")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"network":"esc-mainnet","action":"status"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(lifecycle.status(), StatusCode::OK);
    let lifecycle_body = axum::body::to_bytes(lifecycle.into_body(), usize::MAX)
        .await
        .unwrap();
    let lifecycle_payload: serde_json::Value = serde_json::from_slice(&lifecycle_body).unwrap();
    assert_eq!(lifecycle_payload["status"], "ok");
    assert_eq!(lifecycle_payload["data"]["state"], "managed_local");
    assert_eq!(lifecycle_payload["data"]["control_available"], true);
    assert!(lifecycle_payload["data"].get("rpc_url").is_none());

    let start = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/chain/node_lifecycle")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"network":"esc-mainnet","action":"start"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(start.status(), StatusCode::OK);
    let start_body = axum::body::to_bytes(start.into_body(), usize::MAX)
        .await
        .unwrap();
    let start_payload: serde_json::Value = serde_json::from_slice(&start_body).unwrap();
    assert_eq!(start_payload["status"], "ok");
    assert_eq!(start_payload["data"]["action"], "start");

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let lifecycle_events: Vec<_> = auth_state
        .audit
        .iter()
        .filter(|event| event.event_type.starts_with("chain.node_lifecycle."))
        .collect();
    assert_eq!(lifecycle_events.len(), 2);
    assert_eq!(
        lifecycle_events[0].event_type,
        "chain.node_lifecycle.requested"
    );
    assert_eq!(lifecycle_events[0].result, "requested");
    assert_eq!(
        lifecycle_events[1].event_type,
        "chain.node_lifecycle.completed"
    );
    assert_eq!(lifecycle_events[1].result, "completed");
    assert_eq!(
        lifecycle_events[0].challenge_id,
        lifecycle_events[1].challenge_id
    );
    assert_ne!(lifecycle_events[0].event_id, lifecycle_events[1].event_id);
    assert_eq!(
        lifecycle_events[0].capsule_id.as_deref(),
        Some(SYSTEM_CAPSULE_ID)
    );
    assert!(lifecycle_events[0].reason.contains("esc-mainnet"));
    assert!(lifecycle_events[0].reason.contains("start"));
}

#[tokio::test]
async fn test_wallet_token_can_read_chain_provider_balance() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(chain_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), WALLET_CAPSULE_ID).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/chain/balance")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"network":"esc-mainnet","address":"0x0000000000000000000000000000000000000001"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["status"], "ok");
    assert_eq!(payload["data"]["balance_hex"], "0xde0b6b3a7640000");
    assert!(payload["data"].get("rpc_url").is_none());
}

#[tokio::test]
async fn test_gateway_blocks_chain_proof_prepare_and_broadcast_routes() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(chain_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();

    for op in ["proof", "prepare_transaction", "broadcast_transaction"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/provider/chain/{op}"))
                    .header("x-elastos-home-token", token.clone())
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
