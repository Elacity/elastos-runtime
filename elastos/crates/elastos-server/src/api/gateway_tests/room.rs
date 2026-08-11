use super::*;

mod direct;

struct MockPeerProvider;

fn room_store_snapshot(data_dir: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    fn collect(
        root: &std::path::Path,
        current: &std::path::Path,
        output: &mut std::collections::BTreeMap<String, Vec<u8>>,
    ) {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(root, &path, output);
            } else if path.is_file() {
                let relative = path.strip_prefix(root).unwrap().display().to_string();
                output.insert(relative, std::fs::read(path).unwrap());
            }
        }
    }

    let root = elastos_common::localhost::rooted_localhost_fs_path(
        data_dir,
        crate::room_service::room_root_uri(),
    )
    .unwrap();
    let mut output = std::collections::BTreeMap::new();
    collect(&root, &root, &mut output);
    output
}

#[async_trait::async_trait]
impl Provider for MockPeerProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock peer provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["elastos"]
    }

    fn name(&self) -> &'static str {
        "mock-peer-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match request.get("op").and_then(|value| value.as_str()) {
            Some("get_ticket") => Ok(json!({
                "status": "ok",
                "data": {
                    "ticket": "gateway-live-ticket",
                    "node_id": "gateway-live-node"
                }
            })),
            Some(other) => Ok(json!({
                "status": "error",
                "message": format!("unknown peer op: {other}")
            })),
            None => Ok(json!({
                "status": "error",
                "message": "missing peer op"
            })),
        }
    }
}

#[tokio::test]
async fn test_room_service_assets_serve() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/apps/chat-room/")
                .header(HOST, "chat-room.localhost:61180")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/html; charset=utf-8")
    );
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("Chat"));

    let wasm = app
        .oneshot(
            Request::builder()
                .uri("/apps/chat-room/chat_room_ui_bg.wasm")
                .header(HOST, "chat-room.localhost:61180")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wasm.status(), StatusCode::OK);
    assert_eq!(
        wasm.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/wasm")
    );
}

#[tokio::test]
async fn test_gateway_carrier_bootstrap_route_returns_live_ticket() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("peer", Arc::new(MockPeerProvider))
        .await
        .unwrap();
    let app = gateway_router(GatewayState {
        provider_registry: Some(registry),
        collaboration_chat_product_port: None,
        collaboration_presence_product_port: None,
        collaboration_discovery_service: None,
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: dir.path().to_path_buf(),
        data_dir: dir.path().to_path_buf(),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/.well-known/elastos/carrier-bootstrap.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["schema"], "elastos.carrier.bootstrap/v1");
    assert_eq!(payload["ticket"], "gateway-live-ticket");
    assert_eq!(payload["node_id"], "gateway-live-node");
}

#[tokio::test]
async fn test_gateway_carrier_bootstrap_prefers_managed_runtime_ticket() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "managed-room-peer").await;
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("peer", Arc::new(MockPeerProvider))
        .await
        .unwrap();
    let app = gateway_router(GatewayState {
        provider_registry: Some(registry),
        collaboration_chat_product_port: None,
        collaboration_presence_product_port: None,
        collaboration_discovery_service: None,
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: dir.path().to_path_buf(),
        data_dir: dir.path().to_path_buf(),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/.well-known/elastos/carrier-bootstrap.json")
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
    assert_eq!(payload["schema"], "elastos.carrier.bootstrap/v1");
    assert_eq!(payload["ticket"], "fake-ticket-managed-room-peer");
    assert_eq!(payload["node_id"], "managed-room-peer");
}

#[tokio::test]
async fn test_gateway_carrier_bootstrap_publisher_role_uses_gateway_ticket() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "managed-room-peer").await;
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("peer", Arc::new(MockPeerProvider))
        .await
        .unwrap();
    let app = gateway_router(GatewayState {
        provider_registry: Some(registry),
        collaboration_chat_product_port: None,
        collaboration_presence_product_port: None,
        collaboration_discovery_service: None,
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: dir.path().to_path_buf(),
        data_dir: dir.path().to_path_buf(),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/.well-known/elastos/carrier-bootstrap.json?role=publisher")
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
    assert_eq!(payload["schema"], "elastos.carrier.bootstrap/v1");
    assert_eq!(payload["ticket"], "gateway-live-ticket");
    assert_eq!(payload["node_id"], "gateway-live-node");
    assert_eq!(payload["role"], "publisher");
}

#[tokio::test]
async fn test_chat_room_summary_is_available_without_shell_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/chat-room/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = summary.status();
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["room_slug"], "chat-room");
    assert!(payload["browser_access_allowed"].is_boolean());
    assert_eq!(payload["transport"]["configured"], false);
    assert_eq!(payload["transport"]["available"], false);
    assert_eq!(
        payload["transport"]["status"],
        "Collaboration is isolated on this Runtime."
    );
}

#[tokio::test]
async fn test_chat_room_session_start_connects_open_room_local_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_profile(dir.path(), "anders");

    let launch = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"chat-room"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap();
    let token = test_launch_token_from_route(route);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
                .header("x-elastos-home-token", token.as_str())
                .body(Body::empty())
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
    assert_eq!(payload["status"], "connected");
    assert_eq!(payload["display_name"], "anders");
}

#[tokio::test]
async fn test_chat_room_join_link_create_returns_elastos_join_object() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_profile(dir.path(), "anders");
    let profile = load_profile_for_authority(dir.path(), &authority);

    let launch = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"chat-room"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap();
    let token = test_launch_token_from_route(route);

    let start = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
                .header("x-elastos-home-token", token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = start.status();
    let body = axum::body::to_bytes(start.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/invites/create-link")
                .header("x-elastos-home-token", token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{}"#))
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
    let invite_url = payload["invite_url"].as_str().unwrap();
    let invite_token = payload["token"].as_str().unwrap();

    assert!(invite_url.starts_with("elastos://peer/invite?token="));
    assert_eq!(
        crate::room_service::room_join_invite_token_from_input(invite_url).unwrap(),
        invite_token
    );
    let (envelope, signer_did) =
        crate::room_service::decode_room_join_invite_token(invite_token).unwrap();
    assert_eq!(payload["room_title"], "Chat");
    assert_eq!(
        payload["invited_by_profile_did"],
        profile.document().profile_did
    );
    assert!(payload.get("issuer_gateway").is_none());
    assert_eq!(envelope.payload.room_title, "Chat");
    assert_eq!(
        envelope.payload.invited_by_profile_did,
        profile.document().profile_did
    );
    assert_ne!(signer_did, profile.document().profile_did);
}

#[tokio::test]
async fn test_chat_room_join_link_create_rejects_caller_supplied_issuer_gateway() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_profile(dir.path(), "anders");

    let launch = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"chat-room"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap();
    let token = test_launch_token_from_route(route);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/invites/create-link")
                .header("x-elastos-home-token", token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"issuer_gateway":"https://elastos.elacitylabs.com"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn test_chat_room_session_start_requires_active_local_member_for_seeded_room() {
    let dir = tempfile::tempdir().unwrap();
    let (_owner_device_did, owner_profile) =
        test_signed_room_profile(dir.path(), 41, "Owner", Some("owner"));
    crate::room_service::seed_room_owner(
        dir.path(),
        &owner_profile,
        crate::room_service::RoomOwnerSeedInput {
            title: "Exclusive Room".to_string(),
        },
    )
    .unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_profile(dir.path(), "anders");

    let launch = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"chat-room"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap();
    let token = test_launch_token_from_route(route);

    let denied = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
                .header("x-elastos-home-token", token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = denied.status();
    let body = axum::body::to_bytes(denied.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn test_chat_room_session_start_connects_active_local_member() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_profile(dir.path(), "anders");
    let profile = load_profile_for_authority(dir.path(), &authority);
    crate::room_service::seed_room_owner(
        dir.path(),
        &profile,
        crate::room_service::RoomOwnerSeedInput {
            title: "Local Room".to_string(),
        },
    )
    .unwrap();
    let app = gateway_router(test_state(dir.path()));

    let launch = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"chat-room"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap();
    let token = test_launch_token_from_route(route);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
                .header("x-elastos-home-token", token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let cookie = room_cookie_header(&response);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert!(cookie.starts_with("room-session="));
    assert_eq!(payload["status"], "connected");
    assert_eq!(payload["display_name"], "anders");
    assert!(payload["poll"]["participants"]
        .as_array()
        .unwrap()
        .iter()
        .all(|participant| participant.get("member_did").is_none()));
}

#[tokio::test]
async fn test_chat_room_configured_send_uses_signed_home_authority_and_scoped_port() {
    let dir = tempfile::tempdir().unwrap();
    let (_owner_device_did, owner_profile) =
        test_signed_room_profile(dir.path(), 42, "Hostile Owner", Some("hostile-owner"));
    let owner_profile_did = owner_profile.document().profile_did.clone();
    crate::room_service::seed_room_owner(
        dir.path(),
        &owner_profile,
        crate::room_service::RoomOwnerSeedInput {
            title: "Hostile legacy room".to_string(),
        },
    )
    .unwrap();
    crate::room_service::update_room_access_policy(
        dir.path(),
        crate::room_service::RoomAccessPolicyUpdateInput {
            actor_did: owner_profile_did,
            allow_guest_invites: false,
            allow_member_invites: false,
            allow_members_to_host_guests: false,
        },
    )
    .unwrap();
    let room_root = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        crate::room_service::room_root_uri(),
    )
    .unwrap();
    let members_path = room_root.join("room/members.json");
    let control_path = room_root.join("room/control.json");
    let objects_path = room_root.join("room/objects.json");
    let members_before = std::fs::read(&members_path).unwrap();
    let control_before = std::fs::read(&control_path).unwrap();
    let unscoped_objects_before =
        serde_json::from_slice::<Vec<serde_json::Value>>(&std::fs::read(&objects_path).unwrap())
            .unwrap()
            .into_iter()
            .filter(|object| object.get("collaboration_scope").is_none())
            .count();
    let authority = passkey_authority_with_profile(dir.path(), "anders");
    let port = crate::collaboration_product::test_chat_product_port(
        dir.path(),
        "route-network",
        "route-conversation",
    );
    let mut state = test_state(dir.path());
    state.collaboration_chat_product_port = Some(port.clone());
    let app = gateway_router(state);
    let token =
        projection_launch_token_for_authority_context(dir.path(), CHAT_ROOM_CAPSULE_ID, &authority);

    for request in [
        Request::builder()
            .uri("/api/apps/chat-room/summary")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .uri("/api/apps/chat-room/summary")
            .header("x-elastos-home-token", "forged")
            .body(Body::empty())
            .unwrap(),
    ] {
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/chat-room/summary")
                .header("x-elastos-home-token", token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let summary_status = summary.status();
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        summary_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&summary_body)
    );
    let summary: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(summary["transport"]["configured"], true);
    assert_eq!(summary["transport"]["available"], true);
    assert!(summary["transport"].get("connected_peer_count").is_none());
    assert!(summary["transport"].get("topic").is_none());
    assert_eq!(summary["browser_access_allowed"], false);
    assert!(summary.get("room_control").is_none());
    assert!(summary.get("pending_requests").is_none());
    assert!(summary.get("active_sessions").is_none());
    assert!(summary.get("canonical_hosted_guest_url").is_none());

    for request in [
        test_browser_request("localhost:61180", "null")
            .method("POST")
            .uri("/api/apps/chat-room/session/start")
            .body(Body::empty())
            .unwrap(),
        test_browser_request("localhost:61180", "null")
            .method("POST")
            .uri("/api/apps/chat-room/session/start")
            .header("x-elastos-home-token", "forged")
            .body(Body::empty())
            .unwrap(),
        test_browser_request("localhost:61180", "null")
            .method("POST")
            .uri("/api/apps/chat-room/poll")
            .header("x-elastos-home-token", "forged")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"since":0}"#))
            .unwrap(),
        test_browser_request("localhost:61180", "null")
            .method("POST")
            .uri("/api/apps/chat-room/session/leave")
            .header("x-elastos-home-token", "forged")
            .body(Body::empty())
            .unwrap(),
    ] {
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    assert!(crate::room_service::load_summary(dir.path())
        .unwrap()
        .active_sessions
        .is_empty());
    let missing_session_poll = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header("x-elastos-home-token", token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_session_poll.status(), StatusCode::UNAUTHORIZED);
    assert!(crate::room_service::load_summary(dir.path())
        .unwrap()
        .active_sessions
        .is_empty());

    let session = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
                .header("x-elastos-home-token", token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(session.status(), StatusCode::OK);
    let room_cookie = room_cookie_header(&session);
    let session_body = axum::body::to_bytes(session.into_body(), usize::MAX)
        .await
        .unwrap();
    let session_payload: serde_json::Value = serde_json::from_slice(&session_body).unwrap();
    assert!(session_payload.get("session_token").is_none());
    assert_eq!(session_payload["poll"]["transport"]["configured"], true);
    assert_eq!(session_payload["poll"]["objects"], json!([]));
    assert_eq!(
        crate::room_service::load_summary(dir.path())
            .unwrap()
            .active_sessions
            .len(),
        1
    );

    let send_request = || {
        test_browser_request("localhost:61180", "null")
            .method("POST")
            .uri("/api/apps/chat-room/objects/send")
            .header("x-elastos-home-token", token.as_str())
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"request_id":"chat-message:00112233445566778899aabbccddeeff","body":"hello from collaboration"}"#,
            ))
            .unwrap()
    };
    let first = app.clone().oneshot(send_request()).await.unwrap();
    let first_status = first.status();
    let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        first_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&first_body)
    );
    let first: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
    let replay = app.clone().oneshot(send_request()).await.unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let replay: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(replay.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(replay["seq"], first["seq"]);
    assert_eq!(port.test_live_unresolved_outgoing().unwrap(), 1);

    let conflicting = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/objects/send")
                .header("x-elastos-home-token", token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"request_id":"chat-message:00112233445566778899aabbccddeeff","body":"changed"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(conflicting.status(), StatusCode::BAD_REQUEST);
    assert_eq!(port.test_live_unresolved_outgoing().unwrap(), 1);

    let poll = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header("x-elastos-home-token", token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(poll.status(), StatusCode::OK);
    let poll: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(poll.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(poll["transport"]["available"], true);
    assert_eq!(poll["transport"]["configured"], true);
    assert!(poll["transport"].get("connected_peer_count").is_none());
    assert!(poll["transport"].get("topic").is_none());
    assert_eq!(poll["objects"].as_array().unwrap().len(), 1);
    assert_eq!(poll["objects"][0]["body"], "hello from collaboration");
    assert_eq!(poll["objects"][0]["from_current_session"], true);

    let cookie_only_summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/chat-room/summary")
                .header(COOKIE, room_cookie.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cookie_only_summary.status(), StatusCode::UNAUTHORIZED);
    let cookie_only_poll = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(COOKIE, room_cookie.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cookie_only_poll.status(), StatusCode::UNAUTHORIZED);
    let cookie_only_leave = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/session/leave")
                .header(COOKIE, room_cookie.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cookie_only_leave.status(), StatusCode::UNAUTHORIZED);
    let direct = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/objects/send")
                .header(COOKIE, room_cookie.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"request_id":"chat-message:direct","body":"not authorized"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(direct.status(), StatusCode::UNAUTHORIZED);
    let forged = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/objects/send")
                .header("x-elastos-home-token", "forged")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"request_id":"chat-message:forged","body":"not authorized"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::UNAUTHORIZED);

    let uploads_path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        crate::room_service::room_root_uri(),
    )
    .unwrap()
    .join("local/uploads.json");
    let uploads_before = std::fs::read(&uploads_path).unwrap();
    let upload = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/upload/start")
                .header("x-elastos-home-token", token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"file_name":"unsupported.txt","mime_type":"text/plain","size_bytes":4}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(upload.status(), StatusCode::CONFLICT);
    assert_eq!(std::fs::read(uploads_path).unwrap(), uploads_before);

    let leave = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/session/leave")
                .header("x-elastos-home-token", token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(leave.status(), StatusCode::OK);
    assert!(crate::room_service::load_summary(dir.path())
        .unwrap()
        .active_sessions
        .is_empty());
    let late_poll = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header("x-elastos-home-token", token.as_str())
                .header(COOKIE, room_cookie.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(late_poll.status(), StatusCode::UNAUTHORIZED);
    assert!(crate::room_service::load_summary(dir.path())
        .unwrap()
        .active_sessions
        .is_empty());
    let other_authority = passkey_authority_with_name_role(
        dir.path(),
        Some("other"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    provision_signed_profile(dir.path(), &other_authority, "other");
    let other_token = projection_launch_token_for_authority_context(
        dir.path(),
        CHAT_ROOM_CAPSULE_ID,
        &other_authority,
    );
    let changed_principal = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/objects/send")
                .header("x-elastos-home-token", other_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"request_id":"chat-message:00112233445566778899aabbccddeeff","body":"hello from collaboration"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(changed_principal.status(), StatusCode::BAD_REQUEST);
    assert_eq!(port.test_live_unresolved_outgoing().unwrap(), 1);
    let other_leave = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/session/leave")
                .header("x-elastos-home-token", other_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(other_leave.status(), StatusCode::OK);
    assert!(crate::room_service::load_summary(dir.path())
        .unwrap()
        .active_sessions
        .is_empty());
    assert_eq!(std::fs::read(members_path).unwrap(), members_before);
    assert_eq!(std::fs::read(control_path).unwrap(), control_before);
    let unscoped_objects_after =
        serde_json::from_slice::<Vec<serde_json::Value>>(&std::fs::read(objects_path).unwrap())
            .unwrap()
            .into_iter()
            .filter(|object| object.get("collaboration_scope").is_none())
            .count();
    assert_eq!(unscoped_objects_after, unscoped_objects_before);
}

#[tokio::test]
async fn configured_chat_rejects_every_legacy_control_and_guest_route_before_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let (_owner_device_did, owner_profile) =
        test_signed_room_profile(dir.path(), 43, "Legacy Owner", Some("legacy-owner"));
    crate::room_service::seed_room_owner(
        dir.path(),
        &owner_profile,
        crate::room_service::RoomOwnerSeedInput {
            title: "Legacy room".to_string(),
        },
    )
    .unwrap();
    let room_root = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        crate::room_service::room_root_uri(),
    )
    .unwrap();
    let attachment_dir = room_root.join("room/attachments");
    std::fs::create_dir_all(&attachment_dir).unwrap();
    std::fs::write(attachment_dir.join("preserve.bin"), b"preserve").unwrap();

    let port = crate::collaboration_product::test_chat_product_port(
        dir.path(),
        "configured-network",
        "configured-conversation",
    );
    let mut state = test_state(dir.path());
    state.collaboration_chat_product_port = Some(port);
    let app = gateway_router(state);
    let before = room_store_snapshot(dir.path());
    let acceptance = serde_json::json!({
        "acceptance": {
            "payload": {
                "schema": "elastos.room.accept.v1",
                "room_slug": "chat-room",
                "room_title": "Legacy room",
                "owner_did": "did:key:z6legacy-owner",
                "current_key_epoch": 1,
                "invite_id": "invite",
                "member_did": "did:key:z6member",
                "role": "member",
                "invited_by": "did:key:z6legacy-owner",
                "accepted_at": 1
            },
            "signature": "invalid",
            "signer_did": "did:key:z6member"
        }
    });
    let requests = vec![
        Request::builder().method("POST").uri("/api/apps/chat-room/requests/request/approve").body(Body::empty()).unwrap(),
        Request::builder().method("POST").uri("/api/apps/chat-room/requests/request/deny").body(Body::empty()).unwrap(),
        Request::builder().method("POST").uri("/api/apps/chat-room/guests/session/kick").body(Body::empty()).unwrap(),
        Request::builder().method("POST").uri("/api/apps/chat-room/access-policy").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"allow_guest_invites":true,"allow_member_invites":true,"allow_members_to_host_guests":true}"#)).unwrap(),
        Request::builder().method("POST").uri("/api/apps/chat-room/members/invite").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"member_did":"did:key:z6member","role":"member"}"#)).unwrap(),
        Request::builder().method("POST").uri("/api/apps/chat-room/members/remove").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"member_did":"did:key:z6member"}"#)).unwrap(),
        Request::builder().method("POST").uri("/api/apps/chat-room/invites/revoke").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"invite_id":"invite"}"#)).unwrap(),
        Request::builder().method("POST").uri("/api/apps/chat-room/invites/create-link").header(CONTENT_TYPE, "application/json").body(Body::from("{}")).unwrap(),
        Request::builder().method("POST").uri("/api/apps/chat-room/invites/claim").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"token":"invite","member_did":"did:key:z6member"}"#)).unwrap(),
        Request::builder().method("POST").uri("/api/apps/chat-room/invites/acceptance").header(CONTENT_TYPE, "application/json").body(Body::from(acceptance.to_string())).unwrap(),
        Request::builder().method("POST").uri("/api/apps/chat-room/invites/join").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"invite":"invite"}"#)).unwrap(),
        Request::builder().method("POST").uri("/api/apps/chat-room/upload/start").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"file_name":"blocked.txt","mime_type":"text/plain","size_bytes":1}"#)).unwrap(),
        Request::builder().method("POST").uri("/api/apps/chat-room/upload/upload/chunk").header("x-elastos-upload-offset", "0").body(Body::from("x")).unwrap(),
        Request::builder().method("POST").uri("/api/apps/chat-room/upload/upload/finish").body(Body::empty()).unwrap(),
        Request::builder().uri("/api/apps/chat-room/attachments/attachment").body(Body::empty()).unwrap(),
        Request::builder().method("POST").uri("/api/browser/session/request").header(CONTENT_TYPE, "application/json").body(Body::from(r#"{"display_name":"Guest","device_label":"Browser","capabilities":["room.access"]}"#)).unwrap(),
        Request::builder().uri("/api/browser/session/request/request").header(COOKIE, "browser-session-request=request").body(Body::empty()).unwrap(),
    ];
    for request in requests {
        let method = request.method().clone();
        let uri = request.uri().to_string();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::CONFLICT,
            "{} {}",
            method,
            uri
        );
    }
    assert_eq!(room_store_snapshot(dir.path()), before);
    assert_eq!(
        std::fs::read(attachment_dir.join("preserve.bin")).unwrap(),
        b"preserve"
    );
}

#[tokio::test]
async fn unconfigured_chat_room_access_policy_uses_strict_post_guard_decoding() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_profile(dir.path(), "anders");
    let profile = load_profile_for_authority(dir.path(), &authority);
    crate::room_service::seed_room_owner(
        dir.path(),
        &profile,
        crate::room_service::RoomOwnerSeedInput {
            title: "Strict Room".to_string(),
        },
    )
    .unwrap();
    let app = gateway_router(test_state(dir.path()));
    let token =
        projection_launch_token_for_authority_context(dir.path(), CHAT_ROOM_CAPSULE_ID, &authority);
    let before = room_store_snapshot(dir.path());

    let invalid = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/access-policy")
                .header("x-elastos-home-token", token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"allow_guest_invites":true,"allow_member_invites":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(room_store_snapshot(dir.path()), before);

    let valid = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/access-policy")
                .header("x-elastos-home-token", token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"allow_guest_invites":false,"allow_member_invites":false,"allow_members_to_host_guests":false}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = valid.status();
    let body = axum::body::to_bytes(valid.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["allow_guest_invites"], false);
    assert_eq!(payload["allow_member_invites"], false);
    assert_eq!(payload["allow_members_to_host_guests"], false);

    let summary = crate::room_service::load_summary(dir.path()).unwrap();
    assert!(!summary.room_control.access_policy.allow_guest_invites);
    assert!(!summary.room_control.access_policy.allow_member_invites);
    assert!(
        !summary
            .room_control
            .access_policy
            .allow_members_to_host_guests
    );
}

#[test]
fn gateway_room_source_has_no_route_owned_resource_bridge() {
    let source = include_str!("../gateway_room.rs");
    for removed in [
        concat!("RoomTransport", "Bridge"),
        concat!("ROOM_", "TRANSPORT_"),
        concat!("room", "-sync"),
        concat!("gossip_", "send"),
        concat!("gossip_", "recv"),
        concat!("append_object", "_with_transport"),
        concat!("leave_session", "_with_transport"),
    ] {
        assert!(!source.contains(removed), "stale route bridge: {removed}");
    }

    let carrier_source = include_str!("../../carrier.rs");
    assert!(
        !carrier_source.contains(concat!("room-sync", "-v1")),
        "retired Chat room-sync topic must be absent from Carrier source"
    );
    let browser_sessions = include_str!("../browser_sessions.rs");
    assert!(
        browser_sessions.contains("configured_collaboration_browser_session_unsupported_response")
    );
    assert!(source.contains("start_configured_chat_room_session"));
    assert!(source.contains("configured_legacy_room_control_unsupported_response"));
}

#[tokio::test]
async fn test_chat_room_shell_can_kick_guest_without_exposing_session_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let chat_token = issue_home_launch_token(dir.path(), CHAT_ROOM_CAPSULE_ID).unwrap();

    let request = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/request")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"display_name":"Guest","device_label":"Browser","capabilities":["room.access"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    assert_eq!(request.status(), StatusCode::OK);
    let body = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let request_id = payload["request_id"].as_str().unwrap();

    let approve = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/chat-room/requests/{request_id}/approve"))
                .header("x-elastos-home-token", &chat_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approve.status(), StatusCode::OK);

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/apps/chat-room/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let session = &payload["active_sessions"][0];
    assert!(session.get("token").is_none());
    let session_id = session["session_id"].as_str().unwrap();

    let kick = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/chat-room/guests/{session_id}/kick"))
                .header("x-elastos-home-token", &chat_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(kick.status(), StatusCode::OK);

    let summary = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/apps/chat-room/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active_session_count"], 0);
}

#[tokio::test]
async fn test_chat_room_cookie_auth_prefers_home_room_session_over_browser_session() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_profile(dir.path(), "anders");

    let launch = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"chat-room"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let home_token = test_launch_token_from_route(payload["route"].as_str().unwrap());

    let native_session = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
                .header("x-elastos-home-token", home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let room_cookie = room_cookie_header(&native_session);

    let browser_request = crate::room_service::request_browser_access(
        dir.path(),
        crate::room_service::BrowserAccessRequestInput {
            display_name: "Browser QA".to_string(),
            device_label: "Incognito".to_string(),
            host_member_did: None,
            capabilities: crate::room_service::room_access_capabilities(),
        },
    )
    .unwrap();
    crate::room_service::approve_next_request(dir.path())
        .unwrap()
        .unwrap();
    let browser_token =
        crate::room_service::browser_access_status(dir.path(), &browser_request.request_id)
            .unwrap()
            .token
            .unwrap();
    let both_cookies = format!("browser-session={browser_token}; {room_cookie}");

    let send = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/objects/send")
                .header(COOKIE, both_cookies)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"request_id":"chat-message:local-home-wins","body":"home identity wins"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(send.status(), StatusCode::OK);

    let poll = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(COOKIE, room_cookie)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let poll_body = axum::body::to_bytes(poll.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&poll_body).unwrap();
    let objects = payload["objects"].as_array().cloned().unwrap_or_default();
    assert!(objects.iter().any(|object| {
        object["kind"].as_str() == Some("text")
            && object["sender"].as_str() == Some("anders")
            && object["body"].as_str() == Some("home identity wins")
    }));
}

#[tokio::test]
async fn test_chat_room_shell_can_approve_browser_access_request() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let launch = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"chat-room"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap();
    let home_token = test_launch_token_from_route(route);

    let request = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/request")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"display_name":"Browser QA","device_label":"Incognito","capabilities":["room.access"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    assert_eq!(request.status(), StatusCode::OK);
    let request_cookie = browser_request_cookie_header(&request);
    let request_body = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .unwrap();
    let request_payload: serde_json::Value = serde_json::from_slice(&request_body).unwrap();
    let request_id = request_payload["request_id"].as_str().unwrap();

    let approve = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/chat-room/requests/{request_id}/approve"))
                .header("x-elastos-home-token", home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = approve.status();
    let approve_body = axum::body::to_bytes(approve.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&approve_body)
    );

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/chat-room/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_payload: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(summary_payload["pending_count"], 0);

    let unbound_status = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/browser/session/request/{request_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unbound_status.status(), StatusCode::FORBIDDEN);

    let approved = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/browser/session/request/{request_id}"))
                .header(COOKIE, request_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let approved_body = axum::body::to_bytes(approved.into_body(), usize::MAX)
        .await
        .unwrap();
    let approved_payload: serde_json::Value = serde_json::from_slice(&approved_body).unwrap();
    assert_eq!(approved_payload["status"], "approved");
}

#[tokio::test]
async fn test_room_service_summary_omits_display_name_suggestion() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/chat-room/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("suggested_display_name").is_none());
}

#[tokio::test]
async fn test_room_service_summary_does_not_create_identity_on_read() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/chat-room/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!dir.path().join("identity").join("device.key").exists());
    assert!(load_home_runtime_coords(dir.path()).is_none());
}

#[tokio::test]
async fn test_room_service_summary_includes_hosted_guest_urls() {
    let dir = tempfile::tempdir().unwrap();
    save_trusted_sources(
        dir.path(),
        &TrustedSourcesConfig {
            schema: "elastos.trusted-sources/v1".to_string(),
            default_source: "default".to_string(),
            sources: vec![TrustedSource {
                name: "default".to_string(),
                publisher_dids: vec![],
                channel: "stable".to_string(),
                discovery_uri: String::new(),
                connect_ticket: String::new(),
                gateways: vec!["https://elastos.elacitylabs.com".to_string()],
                install_path: String::new(),
                installed_version: String::new(),
                head_cid: String::new(),
                publisher_node_id: String::new(),
                ipns_name: String::new(),
            }],
        },
    )
    .unwrap();
    crate::browser_app_hosts::record_ephemeral_browser_app_url(
        dir.path(),
        crate::room_service::room_slug(),
        Some("https://quick.trycloudflare.com"),
    )
    .unwrap();
    let app = gateway_router(test_state(dir.path()));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/chat-room/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["canonical_hosted_guest_url"].as_str(),
        Some("https://elastos.elacitylabs.com/apps/chat-room/")
    );
    assert_eq!(
        json["ephemeral_hosted_guest_url"].as_str(),
        Some("https://quick.trycloudflare.com/")
    );
}

#[tokio::test]
async fn test_room_service_summary_blocks_browser_access_when_seeded_room_has_no_runtime_member() {
    let dir = tempfile::tempdir().unwrap();
    let (_owner_device_did, owner_profile) =
        test_signed_room_profile(dir.path(), 44, "Owner", Some("owner"));
    crate::room_service::seed_room_owner(
        dir.path(),
        &owner_profile,
        crate::room_service::RoomOwnerSeedInput {
            title: "Exec Room".to_string(),
        },
    )
    .unwrap();
    let app = gateway_router(test_state(dir.path()));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/chat-room/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["browser_access_allowed"].as_bool(), Some(false));
    assert!(json["room_control"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .all(|member| member.get("member_did").is_none()));
    assert!(json["room_control"].get("owner_did").is_none());
    assert!(json["browser_access_block_reason"]
        .as_str()
        .unwrap()
        .contains("not part of this conversation"));
}

#[tokio::test]
async fn test_browser_session_request_and_status_routes_chat_room() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let pair_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/request")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"display_name":"Alice","device_label":"Phone","capabilities":["room.access"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    assert_eq!(pair_resp.status(), StatusCode::OK);
    let request_cookie = browser_request_cookie_header(&pair_resp);
    let pair_body = axum::body::to_bytes(pair_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let pair_json: serde_json::Value = serde_json::from_slice(&pair_body).unwrap();
    let request_id = pair_json["request_id"].as_str().unwrap().to_string();

    let status_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/browser/session/request/{}", request_id))
                .header(COOKIE, request_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status_resp.status(), StatusCode::OK);
    let status_body = axum::body::to_bytes(status_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
    assert_eq!(status_json["status"].as_str(), Some("pending"));
}

#[tokio::test]
async fn test_browser_session_pair_is_forbidden_when_seeded_room_has_no_runtime_member() {
    let dir = tempfile::tempdir().unwrap();
    let (_owner_device_did, owner_profile) =
        test_signed_room_profile(dir.path(), 45, "Owner", Some("owner"));
    crate::room_service::seed_room_owner(
        dir.path(),
        &owner_profile,
        crate::room_service::RoomOwnerSeedInput {
            title: "Exec Room".to_string(),
        },
    )
    .unwrap();
    let app = gateway_router(test_state(dir.path()));

    let pair_resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/request")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"display_name":"Alice","device_label":"Phone","capabilities":["room.access"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    assert_eq!(pair_resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_room_service_browser_access_and_object_flow() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let pair_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/request")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"display_name":"Alice","device_label":"Phone","capabilities":["room.access"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    assert_eq!(pair_resp.status(), StatusCode::OK);
    let request_cookie = browser_request_cookie_header(&pair_resp);
    let pair_body = axum::body::to_bytes(pair_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let pair_json: serde_json::Value = serde_json::from_slice(&pair_body).unwrap();
    let request_id = pair_json["request_id"].as_str().unwrap().to_string();

    let status_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/browser/session/request/{}", request_id))
                .header(COOKIE, request_cookie.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status_resp.status(), StatusCode::OK);
    let status_body = axum::body::to_bytes(status_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
    assert_eq!(status_json["status"].as_str(), Some("pending"));

    let approved = crate::room_service::approve_next_request(dir.path())
        .unwrap()
        .unwrap();

    let status_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/browser/session/request/{}", request_id))
                .header(COOKIE, request_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status_resp.status(), StatusCode::OK);
    let room_cookie = browser_cookie_header(&status_resp);
    let status_body = axum::body::to_bytes(status_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
    assert_eq!(status_json["status"].as_str(), Some("approved"));
    assert!(status_json["token"].is_null());

    let send_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/objects/send")
                .header("cookie", &room_cookie)
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"request_id":"chat-message:local-browser","body":"Hello room"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(send_resp.status(), StatusCode::OK);

    let feed_resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header("cookie", &room_cookie)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(feed_resp.status(), StatusCode::OK);
    let feed_body = axum::body::to_bytes(feed_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let feed_json: serde_json::Value = serde_json::from_slice(&feed_body).unwrap();
    assert_eq!(feed_json["latest_seq"].as_u64(), Some(2));
    assert_eq!(feed_json["objects"][0]["kind"].as_str(), Some("system"));
    assert_eq!(
        feed_json["objects"][0]["body"].as_str(),
        Some("joined the room")
    );
    assert_eq!(feed_json["objects"][1]["body"].as_str(), Some("Hello room"));
    assert_eq!(feed_json["objects"][1]["sender"].as_str(), Some("Alice"));
    assert_eq!(feed_json["objects"][1]["kind"].as_str(), Some("text"));
    assert_eq!(approved.display_name, "Alice");
}

#[tokio::test]
async fn test_room_service_attachment_upload_and_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let pair_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/request")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"display_name":"Alice","device_label":"Phone","capabilities":["room.access"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    let request_cookie = browser_request_cookie_header(&pair_resp);
    let pair_body = axum::body::to_bytes(pair_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let pair_json: serde_json::Value = serde_json::from_slice(&pair_body).unwrap();
    let request_id = pair_json["request_id"].as_str().unwrap().to_string();

    crate::room_service::approve_next_request(dir.path())
        .unwrap()
        .unwrap();

    let status_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/browser/session/request/{}", request_id))
                .header(COOKIE, request_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let room_cookie = browser_cookie_header(&status_resp);
    let status_body = axum::body::to_bytes(status_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
    assert!(status_json["token"].is_null());

    let upload_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/upload/start")
                .header("cookie", &room_cookie)
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"file_name":"photo.png","mime_type":"image/png","size_bytes":{}}}"#,
                    b"png-data".len()
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(upload_resp.status(), StatusCode::OK);
    let upload_body = axum::body::to_bytes(upload_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let upload_json: serde_json::Value = serde_json::from_slice(&upload_body).unwrap();
    let upload_id = upload_json["upload_id"].as_str().unwrap();

    let chunk_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/apps/chat-room/upload/{}/chunk", upload_id))
                .header("cookie", &room_cookie)
                .header("x-elastos-upload-offset", "0")
                .header("content-type", "application/octet-stream")
                .body(Body::from("png-data"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(chunk_resp.status(), StatusCode::OK);

    let finish_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/apps/chat-room/upload/{}/finish", upload_id))
                .header("cookie", &room_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(finish_resp.status(), StatusCode::OK);
    let finish_body = axum::body::to_bytes(finish_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let finish_json: serde_json::Value = serde_json::from_slice(&finish_body).unwrap();
    assert_eq!(finish_json["kind"].as_str(), Some("attachment"));
    let attachment_id = finish_json["attachment"]["attachment_id"].as_str().unwrap();

    let fetch_resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/apps/chat-room/attachments/{}", attachment_id))
                .header("cookie", &room_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fetch_resp.status(), StatusCode::OK);
    assert_eq!(
        fetch_resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("image/png")
    );
    let bytes = axum::body::to_bytes(fetch_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"png-data");
}

#[tokio::test]
async fn test_room_service_audio_attachment_upload_is_inline_media() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let pair_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/request")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"display_name":"Alice","device_label":"Phone","capabilities":["room.access"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    let request_cookie = browser_request_cookie_header(&pair_resp);
    let pair_body = axum::body::to_bytes(pair_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let pair_json: serde_json::Value = serde_json::from_slice(&pair_body).unwrap();
    let request_id = pair_json["request_id"].as_str().unwrap().to_string();

    crate::room_service::approve_next_request(dir.path())
        .unwrap()
        .unwrap();

    let status_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/browser/session/request/{}", request_id))
                .header(COOKIE, request_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let room_cookie = browser_cookie_header(&status_resp);
    let status_body = axum::body::to_bytes(status_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
    assert!(status_json["token"].is_null());

    let upload_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/upload/start")
                .header("cookie", &room_cookie)
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"file_name":"voice.ogg","mime_type":"audio/ogg","size_bytes":{}}}"#,
                    b"ogg-data".len()
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(upload_resp.status(), StatusCode::OK);
    let upload_body = axum::body::to_bytes(upload_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let upload_json: serde_json::Value = serde_json::from_slice(&upload_body).unwrap();
    let upload_id = upload_json["upload_id"].as_str().unwrap();

    let chunk_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/apps/chat-room/upload/{}/chunk", upload_id))
                .header("cookie", &room_cookie)
                .header("x-elastos-upload-offset", "0")
                .header("content-type", "application/octet-stream")
                .body(Body::from("ogg-data"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(chunk_resp.status(), StatusCode::OK);

    let finish_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/apps/chat-room/upload/{}/finish", upload_id))
                .header("cookie", &room_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(finish_resp.status(), StatusCode::OK);
    let finish_body = axum::body::to_bytes(finish_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let finish_json: serde_json::Value = serde_json::from_slice(&finish_body).unwrap();
    assert_eq!(finish_json["attachment"]["is_audio"].as_bool(), Some(true));
    let attachment_id = finish_json["attachment"]["attachment_id"].as_str().unwrap();

    let fetch_resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/apps/chat-room/attachments/{}", attachment_id))
                .header("cookie", &room_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fetch_resp.status(), StatusCode::OK);
    assert_eq!(
        fetch_resp
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok()),
        Some("inline; filename=\"voice.ogg\"")
    );
}

#[tokio::test]
async fn test_room_service_session_leave_appends_system_object() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let pair_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/request")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"display_name":"Alice","device_label":"Phone","capabilities":["room.access"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    let request_cookie = browser_request_cookie_header(&pair_resp);
    let pair_body = axum::body::to_bytes(pair_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let pair_json: serde_json::Value = serde_json::from_slice(&pair_body).unwrap();
    let request_id = pair_json["request_id"].as_str().unwrap().to_string();

    crate::room_service::approve_next_request(dir.path())
        .unwrap()
        .unwrap();

    let status_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/browser/session/request/{}", request_id))
                .header(COOKIE, request_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let room_cookie = browser_cookie_header(&status_resp);
    let status_body = axum::body::to_bytes(status_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
    assert!(status_json["token"].is_null());

    let leave_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/session/leave")
                .header("cookie", &room_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(leave_resp.status(), StatusCode::OK);
    let leave_body = axum::body::to_bytes(leave_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let leave_json: serde_json::Value = serde_json::from_slice(&leave_body).unwrap();
    assert_eq!(leave_json["kind"].as_str(), Some("system"));
    assert_eq!(leave_json["body"].as_str(), Some("left the room"));

    let summary = crate::room_service::load_summary(dir.path()).unwrap();
    assert_eq!(summary.active_session_count, 0);
}
