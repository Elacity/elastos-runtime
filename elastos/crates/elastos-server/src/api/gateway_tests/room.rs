use super::*;

struct MockPeerProvider;

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

async fn poll_chat_room_until<F>(
    app: Router,
    token: &str,
    predicate: F,
    label: &str,
) -> serde_json::Value
where
    F: Fn(&serde_json::Value) -> bool,
{
    let mut last = serde_json::Value::Null;
    for _ in 0..80 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/chat-room/poll")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"since":0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        last = serde_json::from_slice(&body).unwrap();
        if predicate(&last) {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{label} did not converge; last poll: {last}");
}

async fn summary_chat_room_until<F>(app: Router, predicate: F, label: &str) -> serde_json::Value
where
    F: Fn(&serde_json::Value) -> bool,
{
    let mut last = serde_json::Value::Null;
    for _ in 0..80 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/apps/chat-room/summary")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        last = serde_json::from_slice(&body).unwrap();
        if predicate(&last) {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{label} did not converge; last summary: {last}");
}

async fn wait_for_peer_request<F>(
    runtime: &FakeRuntimeHandle,
    predicate: F,
    label: &str,
) -> serde_json::Value
where
    F: Fn(&serde_json::Value) -> bool,
{
    for _ in 0..80 {
        let requests = runtime.provider_requests.lock().await;
        if let Some(request) = requests.iter().find(|request| predicate(request)) {
            return request.clone();
        }
        drop(requests);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let requests = runtime.provider_requests.lock().await;
    panic!("{label} did not occur; requests: {requests:?}");
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
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: dir.path().to_path_buf(),
        data_dir: dir.path().to_path_buf(),
        audit_log: Arc::new(std::sync::OnceLock::new()),
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
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: dir.path().to_path_buf(),
        data_dir: dir.path().to_path_buf(),
        audit_log: Arc::new(std::sync::OnceLock::new()),
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
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: dir.path().to_path_buf(),
        data_dir: dir.path().to_path_buf(),
        audit_log: Arc::new(std::sync::OnceLock::new()),
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
async fn test_chat_room_transport_uses_home_identity_with_split_managed_runtime_identity() {
    let home_dir = tempfile::tempdir().unwrap();
    let runtime_identity_dir = tempfile::tempdir().unwrap();
    let (_, home_did) = elastos_identity::load_or_create_did(home_dir.path()).unwrap();
    let (_, runtime_did) =
        elastos_identity::load_or_create_did(runtime_identity_dir.path()).unwrap();
    assert_ne!(home_did, runtime_did);

    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let runtime = start_fake_runtime_with_identity_dir(
        home_dir.path(),
        runtime_identity_dir.path(),
        bus,
        "split-identity-peer",
    )
    .await;

    crate::room_service::seed_room_owner(
        home_dir.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: home_did.clone(),
            title: "Split Identity Room".to_string(),
        },
    )
    .unwrap();
    let room_token = crate::room_service::start_local_runtime_session(
        home_dir.path(),
        &home_did,
        "Owner",
        "Home",
    )
    .unwrap()
    .token;
    let app = gateway_router(test_state(home_dir.path()));

    let send = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/objects/send")
                .header(AUTHORIZATION, format!("Bearer {room_token}"))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"body":"split identity transport check"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = send.status();
    let body = axum::body::to_bytes(send.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let sent = wait_for_peer_request(
        &runtime,
        |request| request["scheme"] == "peer" && request["op"] == "gossip_send",
        "room send Carrier broadcast",
    )
    .await;
    assert_eq!(sent["body"]["sender_id"].as_str(), Some(home_did.as_str()));
    assert_ne!(
        sent["body"]["sender_id"].as_str(),
        Some(runtime_did.as_str())
    );

    let message = sent["body"]["message"].as_str().unwrap();
    let ts = sent["body"]["ts"].as_u64().unwrap();
    let signature = sent["body"]["signature"].as_str().unwrap();
    let payload_hex = elastos_common::chat_protocol::signing_payload_hex(&home_did, ts, message);
    let payload = hex::decode(payload_hex).unwrap();
    let sig_bytes = hex::decode(signature).unwrap();
    let signature = ed25519_dalek::Signature::from_slice(&sig_bytes).unwrap();
    crate::crypto::decode_did_key(&home_did)
        .unwrap()
        .verify(&payload, &signature)
        .unwrap();
    assert!(crate::crypto::decode_did_key(&runtime_did)
        .unwrap()
        .verify(&payload, &signature)
        .is_err());
}

#[tokio::test]
async fn test_chat_room_poll_does_not_rebroadcast_local_backlog() {
    let dir = tempfile::tempdir().unwrap();
    let (_, did) = elastos_identity::load_or_create_did(dir.path()).unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let runtime = start_fake_runtime(dir.path(), bus, "poll-no-replay-peer").await;

    crate::room_service::seed_room_owner(
        dir.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: did.clone(),
            title: "No Replay Room".to_string(),
        },
    )
    .unwrap();
    let room_token =
        crate::room_service::start_local_runtime_session(dir.path(), &did, "Owner", "Home")
            .unwrap()
            .token;
    let appended = crate::room_service::append_object_with_transport(
        dir.path(),
        &room_token,
        "local backlog should not replay",
    )
    .unwrap();
    assert!(appended.transport_envelope.is_some());

    let app = gateway_router(test_state(dir.path()));
    let poll = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(AUTHORIZATION, format!("Bearer {room_token}"))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = poll.status();
    let body = axum::body::to_bytes(poll.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let requests = runtime.provider_requests.lock().await;
    assert!(
        !requests
            .iter()
            .any(|request| request["scheme"] == "peer" && request["op"] == "gossip_send"),
        "poll should receive from Carrier, not rebroadcast stale local history"
    );
}

#[tokio::test]
async fn test_chat_room_transport_uses_trusted_source_bootstrap() {
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
                connect_ticket: "trusted-source-ticket".to_string(),
                gateways: vec![],
                install_path: String::new(),
                installed_version: String::new(),
                head_cid: String::new(),
                publisher_node_id: "trusted-source-peer".to_string(),
                ipns_name: String::new(),
            }],
        },
    )
    .unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let runtime = start_fake_runtime(dir.path(), bus, "trusted-source-client").await;
    let app = gateway_router(test_state(dir.path()));

    let summary = summary_chat_room_until(
        app,
        |summary| summary["transport"]["connected_peer_count"] == 1,
        "trusted-source bootstrap",
    )
    .await;
    assert_eq!(summary["transport"]["connected_peer_count"], 1);
    let summary_text = summary.to_string();
    assert!(
        !summary_text.contains("trusted-source-ticket"),
        "Chat Room summary must not expose raw trusted-source ticket authority"
    );
    assert!(
        !summary_text.contains("connect_ticket"),
        "Chat Room summary must not expose trusted-source connect_ticket fields"
    );
    let _ = wait_for_peer_request(
        &runtime,
        |request| {
            request["scheme"] == "peer"
                && request["op"] == "remember_peer"
                && request["body"]["ticket"] == "trusted-source-ticket"
        },
        "trusted-source Carrier peer remember",
    )
    .await;
    let _ = wait_for_peer_request(
        &runtime,
        |request| {
            request["scheme"] == "peer"
                && request["op"] == "gossip_join"
                && request["body"]["topic"] == "__elastos_internal/room-sync-v1/chat-room"
                && request["body"]["mode"] == "direct"
        },
        "trusted-source direct topic join",
    )
    .await;
    let _ = wait_for_peer_request(
        &runtime,
        |request| {
            request["scheme"] == "peer"
                && request["op"] == "gossip_join_peers"
                && request["body"]["topic"] == "__elastos_internal/room-sync-v1/chat-room"
                && request["body"]["peers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|peer| peer == "trusted-source-peer")
        },
        "trusted-source topic peer join",
    )
    .await;
}

#[tokio::test]
async fn test_chat_room_transport_prefers_live_gateway_bootstrap() {
    let bootstrap_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bootstrap_addr = bootstrap_listener.local_addr().unwrap();
    let bootstrap_server = tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/.well-known/elastos/carrier-bootstrap.json",
            get(|| async {
                axum::Json(json!({
                    "schema": "elastos.carrier.bootstrap/v1",
                    "transport": "carrier",
                    "ticket": "fresh-gateway-ticket",
                    "node_id": "fresh-gateway-node"
                }))
            }),
        );
        axum::serve(bootstrap_listener, app).await.unwrap();
    });

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
                connect_ticket: "stale-stamped-ticket".to_string(),
                gateways: vec![format!("http://{bootstrap_addr}")],
                install_path: String::new(),
                installed_version: String::new(),
                head_cid: String::new(),
                publisher_node_id: "stale-source-peer".to_string(),
                ipns_name: String::new(),
            }],
        },
    )
    .unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let runtime = start_fake_runtime(dir.path(), bus, "live-bootstrap-client").await;
    let app = gateway_router(test_state(dir.path()));

    let _ = summary_chat_room_until(
        app,
        |summary| summary["transport"]["connected_peer_count"] == 1,
        "live gateway bootstrap",
    )
    .await;
    let _ = wait_for_peer_request(
        &runtime,
        |request| {
            request["scheme"] == "peer"
                && request["op"] == "remember_peer"
                && request["body"]["ticket"] == "fresh-gateway-ticket"
        },
        "live gateway Carrier peer remember",
    )
    .await;
    let requests = runtime.provider_requests.lock().await;
    assert!(!requests.iter().any(|request| {
        request["scheme"] == "peer"
            && request["op"] == "remember_peer"
            && request["body"]["ticket"] == "stale-stamped-ticket"
    }));
    bootstrap_server.abort();
}

#[tokio::test]
async fn test_chat_room_transport_joins_bootstrap_peer_after_topic_already_joined() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let runtime = start_fake_runtime(dir.path(), bus, "late-bootstrap-client").await;
    let app = gateway_router(test_state(dir.path()));

    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/chat-room/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let _ = wait_for_peer_request(
        &runtime,
        |request| {
            request["scheme"] == "peer"
                && request["op"] == "gossip_join"
                && request["body"]["mode"] == "dht"
        },
        "initial DHT topic join",
    )
    .await;

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
                connect_ticket: "trusted-source-ticket".to_string(),
                gateways: vec![],
                install_path: String::new(),
                installed_version: String::new(),
                head_cid: String::new(),
                publisher_node_id: "trusted-source-peer".to_string(),
                ipns_name: String::new(),
            }],
        },
    )
    .unwrap();

    let _ = wait_for_peer_request(
        &runtime,
        |request| {
            request["scheme"] == "peer"
                && request["op"] == "gossip_join"
                && request["body"]["mode"] == "direct"
        },
        "late direct topic join",
    )
    .await;
    let _ = wait_for_peer_request(
        &runtime,
        |request| {
            request["scheme"] == "peer"
                && request["op"] == "gossip_join_peers"
                && request["body"]["peers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|peer| peer == "trusted-source-peer")
        },
        "late trusted-source topic peer join",
    )
    .await;
    let summary = summary_chat_room_until(
        app,
        |summary| summary["transport"]["connected_peer_count"] == 1,
        "late trusted-source bootstrap",
    )
    .await;
    assert_eq!(summary["transport"]["connected_peer_count"], 1);
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
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "join-link-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));

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
    assert_eq!(payload["issuer_gateway"], "https://elastos.elacitylabs.com");
    assert_eq!(payload["room_title"], "Chat");
    assert_eq!(payload["invited_by"], signer_did);
    assert_eq!(
        envelope.payload.issuer_gateway,
        "https://elastos.elacitylabs.com"
    );
    assert_eq!(envelope.payload.room_title, "Chat");
    assert_eq!(envelope.payload.invited_by, signer_did);
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

    let send = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/chat-room/objects/send")
                .header("x-elastos-home-token", token.as_str())
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
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "room-cookie-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));

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

    let owner_session = crate::room_service::start_local_runtime_session_with_transport(
        owner_dir.path(),
        &owner_did,
        "Owner",
        "WSL",
    )
    .unwrap();
    let owner_token = owner_session.session.token.clone();

    let guest_session = crate::room_service::start_local_runtime_session_with_transport(
        guest_dir.path(),
        &guest_did,
        "Guest",
        "Jetson",
    )
    .unwrap();
    let guest_token = guest_session.session.token.clone();

    let owner_state = test_state(owner_dir.path());
    let guest_state = test_state(guest_dir.path());
    let _ =
        super::gateway_room::room_transport_view(&owner_state, owner_session.transport_envelope)
            .await;
    let _ =
        super::gateway_room::room_transport_view(&guest_state, guest_session.transport_envelope)
            .await;
    let owner_gateway = gateway_router(owner_state.clone());
    let guest_gateway = gateway_router(guest_state.clone());

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

    let poll = poll_chat_room_until(
        guest_gateway.clone(),
        &guest_token,
        |poll| {
            poll["transport"]["connected_peer_count"].as_u64() == Some(1)
                && poll["objects"].as_array().is_some_and(|objects| {
                    objects
                        .iter()
                        .any(|object| object["body"].as_str() == Some("hello across runtimes"))
                })
        },
        "cross-runtime message sync",
    )
    .await;
    assert_eq!(poll["transport"]["connected_peer_count"].as_u64(), Some(1));
    assert!(poll["transport"]["status"]
        .as_str()
        .unwrap_or_default()
        .contains("Carrier conversation sync connected to 1 ElastOS peer"));
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
async fn test_room_service_retries_recent_local_objects_after_missed_carrier_send() {
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
            title: "Retry Room".to_string(),
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

    let owner_session = crate::room_service::start_local_runtime_session_with_transport(
        owner_dir.path(),
        &owner_did,
        "Owner",
        "WSL",
    )
    .unwrap();
    let owner_token = owner_session.session.token.clone();

    let guest_session = crate::room_service::start_local_runtime_session_with_transport(
        guest_dir.path(),
        &guest_did,
        "Guest",
        "Jetson",
    )
    .unwrap();
    let guest_token = guest_session.session.token.clone();

    let owner_state = test_state(owner_dir.path());
    let guest_state = test_state(guest_dir.path());
    let _ =
        super::gateway_room::room_transport_view(&owner_state, owner_session.transport_envelope)
            .await;
    let _ =
        super::gateway_room::room_transport_view(&guest_state, guest_session.transport_envelope)
            .await;
    let owner_gateway = gateway_router(owner_state.clone());
    let guest_gateway = gateway_router(guest_state.clone());

    let marker = "carrier retry after missed accepted send";
    {
        let mut bus = bus.lock().await;
        bus.drop_remote_message_substrings.push(marker.to_string());
    }

    let send_response = owner_gateway
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/objects/send")
                .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"body":"{marker}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(send_response.status(), StatusCode::OK);

    let poll = poll_chat_room_until(
        guest_gateway.clone(),
        &guest_token,
        |poll| {
            poll["objects"].as_array().is_some_and(|objects| {
                objects
                    .iter()
                    .any(|object| object["body"].as_str() == Some(marker))
            })
        },
        "missed Carrier send retry",
    )
    .await;
    assert!(poll["objects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|object| { object["body"].as_str() == Some(marker) }));
}

#[tokio::test]
async fn test_room_transport_stops_send_batch_after_local_only_carrier_result() {
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
                connect_ticket: "trusted-source-ticket".to_string(),
                gateways: vec![],
                install_path: String::new(),
                installed_version: String::new(),
                head_cid: String::new(),
                publisher_node_id: "trusted-source-peer".to_string(),
                ipns_name: String::new(),
            }],
        },
    )
    .unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let runtime = start_fake_runtime(dir.path(), bus.clone(), "owner-peer").await;
    let owner_did = elastos_identity::load_or_create_did(dir.path()).unwrap().1;
    crate::room_service::seed_room_owner(
        dir.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: owner_did.clone(),
            title: "Bounded Send Room".to_string(),
        },
    )
    .unwrap();
    let session = crate::room_service::start_local_runtime_session_with_transport(
        dir.path(),
        &owner_did,
        "Owner",
        "WSL",
    )
    .unwrap();
    let token = session.session.token.clone();
    let state = test_state(dir.path());
    let app = gateway_router(state.clone());
    let _ = super::gateway_room::room_transport_view(&state, session.transport_envelope).await;
    let _ = summary_chat_room_until(
        app.clone(),
        |summary| summary["transport"]["connected_peer_count"] == 1,
        "bounded send peer bootstrap",
    )
    .await;

    runtime.provider_requests.lock().await.clear();
    let first = "local-only carrier send should stop batch";
    let second = "queued message must wait after local-only";
    {
        let mut bus = bus.lock().await;
        bus.local_only_message_substrings.push(first.to_string());
    }

    for body in [first, second] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/chat-room/objects/send")
                    .header(AUTHORIZATION, format!("Bearer {}", token))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"body":"{body}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    wait_for_peer_request(
        &runtime,
        |request| {
            request["scheme"] == "peer"
                && request["op"] == "gossip_send"
                && request["body"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(first))
        },
        "first local-only Carrier send",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let requests = runtime.provider_requests.lock().await;
    assert!(
        !requests.iter().any(|request| {
            request["scheme"] == "peer"
                && request["op"] == "gossip_send"
                && request["body"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(second))
        }),
        "bridge should stop the send batch after the first local-only Carrier result"
    );
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

    let owner_session = crate::room_service::start_local_runtime_session_with_transport(
        owner_dir.path(),
        &owner_did,
        "Owner",
        "WSL",
    )
    .unwrap();
    let owner_token = owner_session.session.token.clone();

    let guest_session = crate::room_service::start_local_runtime_session_with_transport(
        guest_dir.path(),
        &guest_did,
        "Guest",
        "Jetson",
    )
    .unwrap();
    let guest_token = guest_session.session.token.clone();

    let owner_state = test_state(owner_dir.path());
    let guest_state = test_state(guest_dir.path());
    let _ =
        super::gateway_room::room_transport_view(&owner_state, owner_session.transport_envelope)
            .await;
    let _ =
        super::gateway_room::room_transport_view(&guest_state, guest_session.transport_envelope)
            .await;
    let owner_gateway = gateway_router(owner_state.clone());
    let guest_gateway = gateway_router(guest_state.clone());

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

    let poll = poll_chat_room_until(
        guest_gateway.clone(),
        &guest_token,
        |poll| {
            poll["objects"].as_array().is_some_and(|objects| {
                objects
                    .iter()
                    .any(|object| object["kind"].as_str() == Some("attachment"))
            })
        },
        "cross-runtime attachment sync",
    )
    .await;
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
async fn test_room_service_replays_durable_local_objects_over_carrier() {
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

    let owner_session = crate::room_service::start_local_runtime_session_with_transport(
        owner_dir.path(),
        &owner_did,
        "Owner",
        "WSL",
    )
    .unwrap();
    let owner_token = owner_session.session.token.clone();
    let guest_session = crate::room_service::start_local_runtime_session_with_transport(
        guest_dir.path(),
        &guest_did,
        "Guest",
        "Mac",
    )
    .unwrap();
    let guest_token = guest_session.session.token.clone();

    crate::room_service::append_object(
        owner_dir.path(),
        &owner_token,
        "durable object without in-memory enqueue",
    )
    .unwrap();

    let owner_state = test_state(owner_dir.path());
    let guest_state = test_state(guest_dir.path());
    let _ = super::gateway_room::room_transport_view(&owner_state, None).await;
    let _ =
        super::gateway_room::room_transport_view(&guest_state, guest_session.transport_envelope)
            .await;
    let guest_gateway = gateway_router(guest_state.clone());

    let poll = poll_chat_room_until(
        guest_gateway,
        &guest_token,
        |poll| {
            poll["objects"].as_array().is_some_and(|objects| {
                objects.iter().any(|object| {
                    object["body"].as_str() == Some("durable object without in-memory enqueue")
                })
            })
        },
        "durable room object replay",
    )
    .await;
    assert!(poll["objects"].as_array().unwrap().iter().any(|object| {
        object["body"].as_str() == Some("durable object without in-memory enqueue")
    }));
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

    let owner_session = crate::room_service::start_local_runtime_session_with_transport(
        owner_dir.path(),
        &owner_did,
        "Owner",
        "WSL",
    )
    .unwrap();
    let owner_token = owner_session.session.token.clone();

    let guest_session = crate::room_service::start_local_runtime_session_with_transport(
        guest_dir.path(),
        &guest_did,
        "Guest",
        "Jetson",
    )
    .unwrap();
    let guest_token = guest_session.session.token.clone();

    let owner_state = test_state(owner_dir.path());
    let guest_state = test_state(guest_dir.path());
    let _ =
        super::gateway_room::room_transport_view(&owner_state, owner_session.transport_envelope)
            .await;
    let _ =
        super::gateway_room::room_transport_view(&guest_state, guest_session.transport_envelope)
            .await;
    let owner_gateway = gateway_router(owner_state.clone());
    let guest_gateway = gateway_router(guest_state.clone());

    let guest_poll = poll_chat_room_until(
        guest_gateway.clone(),
        &guest_token,
        |poll| {
            poll["objects"].as_array().is_some_and(|objects| {
                objects.iter().any(|object| {
                    object["kind"].as_str() == Some("system")
                        && object["sender"].as_str() == Some("Owner")
                        && object["body"].as_str() == Some("joined the room")
                })
            })
        },
        "guest sees owner join",
    )
    .await;
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

    let owner_poll = poll_chat_room_until(
        owner_gateway.clone(),
        &owner_token,
        |poll| {
            poll["objects"].as_array().is_some_and(|objects| {
                objects.iter().any(|object| {
                    object["kind"].as_str() == Some("system")
                        && object["sender"].as_str() == Some("Guest")
                        && object["body"].as_str() == Some("joined the room")
                })
            })
        },
        "owner sees guest join",
    )
    .await;
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

    let owner_after_leave_json = poll_chat_room_until(
        owner_gateway.clone(),
        &owner_token,
        |poll| {
            poll["objects"].as_array().is_some_and(|objects| {
                objects.iter().any(|object| {
                    object["kind"].as_str() == Some("system")
                        && object["sender"].as_str() == Some("Guest")
                        && object["body"].as_str() == Some("left the room")
                })
            }) && poll["participants"].as_array().is_some_and(|participants| {
                participants.len() == 1
                    && participants.iter().any(|participant| {
                        participant["member_did"].as_str() == Some(owner_did.as_str())
                            && participant["display_name"].as_str() == Some("Owner")
                    })
            })
        },
        "owner sees guest leave",
    )
    .await;
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
