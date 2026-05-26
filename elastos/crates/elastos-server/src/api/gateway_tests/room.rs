use super::*;

#[tokio::test]
async fn test_room_service_assets_serve() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/apps/chat-room/")
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
    assert!(text.contains("Chat Room"));

    let wasm = app
        .oneshot(
            Request::builder()
                .uri("/apps/chat-room/chat_room_ui_bg.wasm")
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
async fn test_chat_room_summary_is_available_without_shell_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "summary-peer").await;
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
}

#[tokio::test]
async fn test_chat_room_session_start_connects_open_room_local_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "open-room-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));

    let launch = app
        .clone()
        .oneshot(
            Request::builder()
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
    let token = route.split("home_token=").nth(1).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
                .header("x-elastos-home-token", token)
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
async fn test_chat_room_session_start_requires_active_local_member_for_seeded_room() {
    let dir = tempfile::tempdir().unwrap();
    crate::room_service::seed_room_owner(
        dir.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: "did:key:z6seededowner".to_string(),
            title: "Exclusive Room".to_string(),
        },
    )
    .unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "seeded-room-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));

    let launch = app
        .clone()
        .oneshot(
            Request::builder()
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
    let token = route.split("home_token=").nth(1).unwrap();

    let denied = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
                .header("x-elastos-home-token", token)
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
    let (_, did) = elastos_identity::load_or_create_did(dir.path()).unwrap();
    crate::room_service::seed_room_owner(
        dir.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: did.clone(),
            title: "Local Room".to_string(),
        },
    )
    .unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "active-room-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));

    let launch = app
        .clone()
        .oneshot(
            Request::builder()
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
    let token = route.split("home_token=").nth(1).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
                .header("x-elastos-home-token", token)
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
}

#[tokio::test]
async fn test_chat_room_shell_requests_use_shell_launch_authority_without_room_cookie() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "shell-room-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));

    let launch = app
        .clone()
        .oneshot(
            Request::builder()
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
    let token = route.split("home_token=").nth(1).unwrap();

    let send = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/objects/send")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"body":"hello from shell"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = send.status();
    let send_body = axum::body::to_bytes(send.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&send_body)
    );

    let poll = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = poll.status();
    let poll_body = axum::body::to_bytes(poll.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&poll_body)
    );
    let payload: serde_json::Value = serde_json::from_slice(&poll_body).unwrap();
    let objects = payload["objects"].as_array().cloned().unwrap_or_default();
    assert!(objects.iter().any(|object| {
        object["kind"].as_str() == Some("text")
            && object["sender"].as_str() == Some("anders")
            && object["body"].as_str() == Some("hello from shell")
    }));
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
            Request::builder()
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
            Request::builder()
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
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "room-cookie-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));

    let launch = app
        .clone()
        .oneshot(
            Request::builder()
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
    let home_token = payload["route"]
        .as_str()
        .unwrap()
        .split("home_token=")
        .nth(1)
        .unwrap();

    let native_session = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
                .header("x-elastos-home-token", home_token)
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
                .body(Body::from(r#"{"body":"home identity wins"}"#))
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
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "approve-browser-peer").await;
    let app = gateway_router(test_state(dir.path()));

    let launch = app
        .clone()
        .oneshot(
            Request::builder()
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
    let home_token = route.split("home_token=").nth(1).unwrap();

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
            Request::builder()
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
    crate::room_service::seed_room_owner(
        dir.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: "did:key:z6owner".to_string(),
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
    assert_eq!(json["owner_did"].as_str(), None);
    assert!(json["browser_access_block_reason"]
        .as_str()
        .unwrap()
        .contains("no active room member DID available"));
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
    crate::room_service::seed_room_owner(
        dir.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: "did:key:z6owner".to_string(),
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
                .body(Body::from(r#"{"body":"Hello room"}"#))
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

#[tokio::test]
async fn test_room_service_cross_runtime_room_syncs_over_carrier() {
    let owner_dir = tempfile::tempdir().unwrap();
    let guest_dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let owner_runtime = start_fake_runtime(owner_dir.path(), bus.clone(), "owner-peer").await;
    let guest_runtime = start_fake_runtime(guest_dir.path(), bus.clone(), "guest-peer").await;

    assert!(owner_runtime.api_url.starts_with("http://127.0.0.1:"));
    assert!(guest_runtime.api_url.starts_with("http://127.0.0.1:"));

    let owner_did = elastos_identity::load_or_create_did(owner_dir.path())
        .unwrap()
        .1;
    let guest_did = elastos_identity::load_or_create_did(guest_dir.path())
        .unwrap()
        .1;

    let _ = crate::room_service::seed_room_owner(
        owner_dir.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: owner_did.clone(),
            title: "Exec Room".to_string(),
        },
    )
    .unwrap();
    let invite = crate::room_service::export_room_invite_envelope(
        owner_dir.path(),
        crate::room_service::RoomInviteInput {
            actor_did: owner_did.clone(),
            invited_did: guest_did.clone(),
            role: crate::room_service::RoomRole::Member,
        },
    )
    .unwrap();
    let invite_json = serde_json::to_vec(&invite).unwrap();
    crate::room_service::import_room_invite_envelope(guest_dir.path(), &invite_json).unwrap();
    crate::room_service::accept_room_invite(
        guest_dir.path(),
        crate::room_service::RoomInviteAcceptInput {
            actor_did: guest_did.clone(),
            invite_id: invite.payload.invite_id.clone(),
        },
    )
    .unwrap();
    let acceptance = crate::room_service::export_room_acceptance_envelope(
        guest_dir.path(),
        &invite.payload.invite_id,
    )
    .unwrap();
    let acceptance_json = serde_json::to_vec(&acceptance).unwrap();
    crate::room_service::import_room_acceptance_envelope(owner_dir.path(), &acceptance_json)
        .unwrap();

    let owner_token = crate::room_service::start_local_runtime_session(
        owner_dir.path(),
        &owner_did,
        "Owner",
        "WSL",
    )
    .unwrap()
    .token;

    let guest_token = crate::room_service::start_local_runtime_session(
        guest_dir.path(),
        &guest_did,
        "Guest",
        "Jetson",
    )
    .unwrap()
    .token;

    let owner_gateway = gateway_router(test_state(owner_dir.path()));
    let guest_gateway = gateway_router(test_state(guest_dir.path()));

    let send_response = owner_gateway
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/objects/send")
                .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"body":"hello across runtimes"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(send_response.status(), StatusCode::OK);

    let poll_response = guest_gateway
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(AUTHORIZATION, format!("Bearer {}", guest_token))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(poll_response.status(), StatusCode::OK);
    let poll_body = axum::body::to_bytes(poll_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let poll: serde_json::Value = serde_json::from_slice(&poll_body).unwrap();
    assert_eq!(poll["transport"]["connected_peer_count"].as_u64(), Some(1));
    assert!(poll["transport"]["status"]
        .as_str()
        .unwrap_or_default()
        .contains("Carrier room sync connected to 1 runtime"));
    let participants = poll["participants"].as_array().cloned().unwrap_or_default();
    assert_eq!(participants.len(), 2);
    assert!(participants.iter().any(|participant| {
        participant["member_did"].as_str() == Some(owner_did.as_str())
            && participant["display_name"].as_str() == Some("Owner")
    }));
    assert!(participants.iter().any(|participant| {
        participant["member_did"].as_str() == Some(guest_did.as_str())
            && participant["display_name"].as_str() == Some("Guest")
    }));
    let objects = poll["objects"].as_array().cloned().unwrap_or_default();
    assert!(objects
        .iter()
        .any(|object| object["body"].as_str() == Some("hello across runtimes")));
}

#[tokio::test]
async fn test_room_service_cross_runtime_attachment_syncs_over_carrier() {
    let owner_dir = tempfile::tempdir().unwrap();
    let guest_dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _owner_runtime = start_fake_runtime(owner_dir.path(), bus.clone(), "owner-peer").await;
    let _guest_runtime = start_fake_runtime(guest_dir.path(), bus.clone(), "guest-peer").await;

    let owner_did = elastos_identity::load_or_create_did(owner_dir.path())
        .unwrap()
        .1;
    let guest_did = elastos_identity::load_or_create_did(guest_dir.path())
        .unwrap()
        .1;

    let _ = crate::room_service::seed_room_owner(
        owner_dir.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: owner_did.clone(),
            title: "Exec Room".to_string(),
        },
    )
    .unwrap();
    let invite = crate::room_service::export_room_invite_envelope(
        owner_dir.path(),
        crate::room_service::RoomInviteInput {
            actor_did: owner_did.clone(),
            invited_did: guest_did.clone(),
            role: crate::room_service::RoomRole::Member,
        },
    )
    .unwrap();
    let invite_json = serde_json::to_vec(&invite).unwrap();
    crate::room_service::import_room_invite_envelope(guest_dir.path(), &invite_json).unwrap();
    crate::room_service::accept_room_invite(
        guest_dir.path(),
        crate::room_service::RoomInviteAcceptInput {
            actor_did: guest_did.clone(),
            invite_id: invite.payload.invite_id.clone(),
        },
    )
    .unwrap();
    let acceptance = crate::room_service::export_room_acceptance_envelope(
        guest_dir.path(),
        &invite.payload.invite_id,
    )
    .unwrap();
    let acceptance_json = serde_json::to_vec(&acceptance).unwrap();
    crate::room_service::import_room_acceptance_envelope(owner_dir.path(), &acceptance_json)
        .unwrap();

    let owner_token = crate::room_service::start_local_runtime_session(
        owner_dir.path(),
        &owner_did,
        "Owner",
        "WSL",
    )
    .unwrap()
    .token;

    let guest_token = crate::room_service::start_local_runtime_session(
        guest_dir.path(),
        &guest_did,
        "Guest",
        "Jetson",
    )
    .unwrap()
    .token;

    let owner_gateway = gateway_router(test_state(owner_dir.path()));
    let guest_gateway = gateway_router(test_state(guest_dir.path()));

    let start_response = owner_gateway
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/upload/start")
                .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"file_name":"photo.png","mime_type":"image/png","size_bytes":8}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(start_response.status(), StatusCode::OK);
    let start_body = axum::body::to_bytes(start_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let start_json: serde_json::Value = serde_json::from_slice(&start_body).unwrap();
    let upload_id = start_json["upload_id"].as_str().unwrap().to_string();

    let chunk_response = owner_gateway
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/apps/chat-room/upload/{upload_id}/chunk"))
                .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                .header("x-elastos-upload-offset", "0")
                .body(Body::from(Vec::from(&b"png-data"[..])))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(chunk_response.status(), StatusCode::OK);

    let finish_response = owner_gateway
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/apps/chat-room/upload/{upload_id}/finish"))
                .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(finish_response.status(), StatusCode::OK);

    let poll_response = guest_gateway
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(AUTHORIZATION, format!("Bearer {}", guest_token))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(poll_response.status(), StatusCode::OK);
    let poll_body = axum::body::to_bytes(poll_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let poll: serde_json::Value = serde_json::from_slice(&poll_body).unwrap();
    let attachment_object = poll["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|object| object["kind"].as_str() == Some("attachment"))
        .cloned()
        .expect("attachment object");
    let attachment_id = attachment_object["attachment"]["attachment_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        attachment_object["attachment"]["file_name"].as_str(),
        Some("photo.png")
    );
    assert_eq!(
        attachment_object["attachment"]["mime_type"].as_str(),
        Some("image/png")
    );

    let attachment_response = guest_gateway
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/apps/chat-room/attachments/{attachment_id}"))
                .header(AUTHORIZATION, format!("Bearer {}", guest_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(attachment_response.status(), StatusCode::OK);
    assert_eq!(
        attachment_response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );
    let attachment_body = axum::body::to_bytes(attachment_response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(attachment_body.as_ref(), b"png-data");
}

#[tokio::test]
async fn test_room_service_cross_runtime_presence_syncs_join_and_leave() {
    let owner_dir = tempfile::tempdir().unwrap();
    let guest_dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _owner_runtime = start_fake_runtime(owner_dir.path(), bus.clone(), "owner-peer").await;
    let _guest_runtime = start_fake_runtime(guest_dir.path(), bus.clone(), "guest-peer").await;

    let owner_did = elastos_identity::load_or_create_did(owner_dir.path())
        .unwrap()
        .1;
    let guest_did = elastos_identity::load_or_create_did(guest_dir.path())
        .unwrap()
        .1;

    let _ = crate::room_service::seed_room_owner(
        owner_dir.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: owner_did.clone(),
            title: "Exec Room".to_string(),
        },
    )
    .unwrap();
    let invite = crate::room_service::export_room_invite_envelope(
        owner_dir.path(),
        crate::room_service::RoomInviteInput {
            actor_did: owner_did.clone(),
            invited_did: guest_did.clone(),
            role: crate::room_service::RoomRole::Member,
        },
    )
    .unwrap();
    let invite_json = serde_json::to_vec(&invite).unwrap();
    crate::room_service::import_room_invite_envelope(guest_dir.path(), &invite_json).unwrap();
    crate::room_service::accept_room_invite(
        guest_dir.path(),
        crate::room_service::RoomInviteAcceptInput {
            actor_did: guest_did.clone(),
            invite_id: invite.payload.invite_id.clone(),
        },
    )
    .unwrap();
    let acceptance = crate::room_service::export_room_acceptance_envelope(
        guest_dir.path(),
        &invite.payload.invite_id,
    )
    .unwrap();
    let acceptance_json = serde_json::to_vec(&acceptance).unwrap();
    crate::room_service::import_room_acceptance_envelope(owner_dir.path(), &acceptance_json)
        .unwrap();

    let owner_token = crate::room_service::start_local_runtime_session(
        owner_dir.path(),
        &owner_did,
        "Owner",
        "WSL",
    )
    .unwrap()
    .token;

    let guest_token = crate::room_service::start_local_runtime_session(
        guest_dir.path(),
        &guest_did,
        "Guest",
        "Jetson",
    )
    .unwrap()
    .token;

    let owner_gateway = gateway_router(test_state(owner_dir.path()));
    let guest_gateway = gateway_router(test_state(guest_dir.path()));

    let owner_first_poll = owner_gateway
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(owner_first_poll.status(), StatusCode::OK);

    let guest_first_poll = guest_gateway
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(AUTHORIZATION, format!("Bearer {}", guest_token))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_first_poll.status(), StatusCode::OK);
    let guest_body = axum::body::to_bytes(guest_first_poll.into_body(), usize::MAX)
        .await
        .unwrap();
    let guest_poll: serde_json::Value = serde_json::from_slice(&guest_body).unwrap();
    let guest_objects = guest_poll["objects"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(guest_objects.iter().any(|object| {
        object["kind"].as_str() == Some("system")
            && object["sender"].as_str() == Some("Owner")
            && object["body"].as_str() == Some("joined the room")
    }));
    let guest_participants = guest_poll["participants"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(guest_participants.len(), 2);

    let owner_second_poll = owner_gateway
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(owner_second_poll.status(), StatusCode::OK);
    let owner_body = axum::body::to_bytes(owner_second_poll.into_body(), usize::MAX)
        .await
        .unwrap();
    let owner_poll: serde_json::Value = serde_json::from_slice(&owner_body).unwrap();
    let owner_objects = owner_poll["objects"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(owner_objects.iter().any(|object| {
        object["kind"].as_str() == Some("system")
            && object["sender"].as_str() == Some("Guest")
            && object["body"].as_str() == Some("joined the room")
    }));
    let owner_participants = owner_poll["participants"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(owner_participants.len(), 2);

    let guest_leave = guest_gateway
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/session/leave")
                .header(AUTHORIZATION, format!("Bearer {}", guest_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_leave.status(), StatusCode::OK);

    let owner_after_leave = owner_gateway
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(owner_after_leave.status(), StatusCode::OK);
    let owner_after_leave_body = axum::body::to_bytes(owner_after_leave.into_body(), usize::MAX)
        .await
        .unwrap();
    let owner_after_leave_json: serde_json::Value =
        serde_json::from_slice(&owner_after_leave_body).unwrap();
    let owner_after_leave_objects = owner_after_leave_json["objects"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        owner_after_leave_objects.iter().any(|object| {
            object["kind"].as_str() == Some("system")
                && object["sender"].as_str() == Some("Guest")
                && object["body"].as_str() == Some("left the room")
        }),
        "owner after leave poll: {owner_after_leave_json}"
    );
    let owner_after_leave_participants = owner_after_leave_json["participants"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(owner_after_leave_participants.len(), 1);
    assert!(owner_after_leave_participants.iter().any(|participant| {
        participant["member_did"].as_str() == Some(owner_did.as_str())
            && participant["display_name"].as_str() == Some("Owner")
    }));
}
