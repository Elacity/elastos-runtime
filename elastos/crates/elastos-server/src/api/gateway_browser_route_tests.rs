use super::*;

#[tokio::test]
async fn test_browser_app_summary_declares_fail_closed_engine_adapter() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(test_state(dir.path()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/summary")
                .header("x-elastos-home-token", token)
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
    assert_eq!(payload["schema"], "elastos.browser.runtime/v1");
    assert_eq!(payload["app"]["id"], BROWSER_CAPSULE_ID);
    assert_eq!(
        payload["sessions"]["schema"],
        "elastos.browser.session-capacity/v1"
    );
    assert_eq!(payload["sessions"]["active_sessions"], 0);
    assert_eq!(payload["sessions"]["launching_sessions"], 0);
    assert_eq!(payload["sessions"]["capacity_available"], true);
    assert_eq!(payload["engine_adapter"]["status"], "unavailable");
    assert_eq!(payload["engine_adapter"]["mode"], "not_configured");
    assert_eq!(
        payload["engine_adapter"]["stream_session_schema"],
        "elastos.exit.stream-session/v1"
    );
    assert_eq!(
        payload["engine_adapter"]["display_session_schema"],
        "elastos.browser.display-session/v1"
    );
    assert_eq!(payload["engine_adapter"]["byte_transport"], "not_attached");
    assert_eq!(payload["net"]["status"], "fail_closed");
    assert_eq!(payload["net"]["direct_network"], false);
    assert_eq!(payload["wallet_bridge"]["status"], "no_accounts");
}

#[tokio::test]
async fn test_browser_app_summary_reports_registered_net_and_exit_status() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(net_exit_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/summary")
                .header("x-elastos-home-token", token)
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
    assert_eq!(payload["net"]["status"], "fail_closed");
    assert_eq!(payload["net"]["provider"], "elastos://net/*");
    assert_eq!(payload["net"]["direct_network"], false);
    assert_eq!(
        payload["net"]["exit_provider"]["provider"],
        "elastos://exit/*"
    );
    assert_eq!(payload["net"]["exit_provider"]["status"], "fail_closed");
    assert_eq!(payload["net"]["exit_provider"]["direct_network"], false);
}

#[tokio::test]
async fn test_browser_app_summary_reports_registered_engine_adapter_status() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/summary")
                .header("x-elastos-home-token", token)
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
    assert_eq!(payload["engine_adapter"]["status"], "configured");
    assert_eq!(
        payload["engine_adapter"]["provider"],
        "elastos://browser-engine/*"
    );
    assert_eq!(payload["engine_adapter"]["adapter_count"], 1);
    assert_eq!(payload["engine_adapter"]["byte_transport"], "adapter_ipc");
    assert_eq!(
        payload["engine_adapter"]["display_session_schema"],
        "elastos.browser.display-session/v1"
    );
    assert_eq!(
        payload["engine_adapter"]["supported_display_modes"][0],
        "diagnostic_frame"
    );
    assert_eq!(payload["engine_adapter"]["direct_network"], false);
    assert_eq!(payload["engine_adapter"]["wallet_injection"], false);
}

#[tokio::test]
async fn test_browser_app_summary_rejects_missing_authority_status_proofs() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(malformed_browser_summary_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/summary")
                .header("x-elastos-home-token", token)
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
    assert_eq!(
        payload["engine_adapter"]["status"],
        "invalid_provider_status"
    );
    assert_eq!(payload["engine_adapter"]["direct_network"], false);
    assert_eq!(payload["engine_adapter"]["wallet_injection"], false);
    assert!(payload["engine_adapter"]["reason"]
        .as_str()
        .unwrap()
        .contains("direct_network=false proof"));
    assert_eq!(payload["net"]["status"], "invalid_provider_status");
    assert_eq!(payload["net"]["direct_network"], false);
    assert!(payload["net"]["reason"]
        .as_str()
        .unwrap()
        .contains("direct_network=false proof"));
    assert_eq!(
        payload["net"]["exit_provider"]["status"],
        "invalid_provider_status"
    );
    assert_eq!(payload["net"]["exit_provider"]["direct_network"], false);
}

#[tokio::test]
async fn test_browser_open_fails_closed_without_attached_engine_transport() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","reason":"open browser page"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("adapter_ipc"));
}

#[tokio::test]
async fn test_browser_open_rejects_non_http_urls() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"url":"javascript:alert(1)"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("Only http and https"));
}

#[tokio::test]
async fn test_browser_open_returns_forbidden_when_exit_policy_blocks_host() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_policy_blocked_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://whatismyip.com/","reason":"check exit IP"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("whatismyip.com"));
    assert!(message.contains("direct host networking"));
}

#[tokio::test]
async fn test_browser_open_launches_engine_with_attached_stream_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"glidefinance.io","reason":"open browser page","viewport":{"width":900,"height":520},"display_mode":"diagnostic_frame"}"#,
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
    assert_eq!(payload["schema"], "elastos.browser.open-result/v1");
    assert_eq!(payload["url"], "https://glidefinance.io/");
    assert_eq!(payload["target"], "tls://glidefinance.io:443");
    assert_eq!(
        payload["stream_session"]["schema"],
        "elastos.exit.stream-session/v1"
    );
    assert_eq!(payload["stream_session"]["byte_transport"], "adapter_ipc");
    assert!(payload["stream_session"].get("adapter_ipc").is_none());
    assert!(payload["stream_session"].get("relay_ipc").is_none());
    let stream_id = payload["stream_session"]["stream_id"].as_str().unwrap();
    let runtime_stream_path = browser_runtime_stream_socket_path(dir.path(), stream_id).unwrap();
    assert!(runtime_stream_path.exists());
    assert_eq!(
        payload["engine_page"]["schema"],
        "elastos.browser.engine.page/v1"
    );
    assert_eq!(payload["engine_page"]["direct_network"], false);
    assert_eq!(payload["engine_page"]["wallet_injection"], false);
    assert_eq!(
        payload["engine_page"]["display_session"]["schema"],
        "elastos.browser.display-session/v1"
    );
    assert_eq!(
        payload["engine_page"]["display_session"]["mode"],
        "diagnostic_frame"
    );
    assert_eq!(
        payload["engine_page"]["display_session"]["network_mode"],
        "runtime_net_only"
    );
    assert_eq!(payload["engine_page"]["view"]["width"], 900);
    assert_eq!(payload["engine_page"]["view"]["height"], 520);
    let page_id = payload["engine_page"]["page_id"].as_str().unwrap();

    let heartbeat_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/heartbeat"))
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(heartbeat_response.status(), StatusCode::OK);
    let heartbeat_body = axum::body::to_bytes(heartbeat_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let heartbeat: serde_json::Value = serde_json::from_slice(&heartbeat_body).unwrap();
    assert_eq!(heartbeat["schema"], "elastos.browser.page-heartbeat/v1");
    assert_eq!(heartbeat["page_id"], page_id);

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let open_events: Vec<_> = auth_state
        .audit
        .iter()
        .filter(|event| {
            event.capsule_id.as_deref() == Some(BROWSER_CAPSULE_ID)
                && event.event_type.starts_with("browser.open.")
        })
        .collect();
    assert_eq!(
        open_events.len(),
        2,
        "each successful Browser open must emit requested + completed audit events"
    );
    assert!(
        open_events
            .iter()
            .any(|event| event.event_type == "browser.open.requested"
                && event.result == "requested"
                && event.reason.contains("decision=runtime_net_exit_policy")),
        "Browser opens must record the Net/Exit standing policy decision"
    );
    assert!(
        open_events
            .iter()
            .any(|event| event.event_type == "browser.open.completed"
                && event.result == "allowed"
                && event.reason.contains("decision=browser_engine_provider")),
        "Browser opens must record provider-mediated execution"
    );
}

#[tokio::test]
async fn test_browser_session_capacity_tracks_open_heartbeat_and_close() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let before_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/summary")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(before_response.status(), StatusCode::OK);
    let before_body = axum::body::to_bytes(before_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let before: serde_json::Value = serde_json::from_slice(&before_body).unwrap();
    assert_eq!(before["sessions"]["principal_sessions"], 0);

    let open_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://example.com/","reason":"session capacity regression","viewport":{"width":900,"height":520},"display_mode":"diagnostic_frame"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(open_response.status(), StatusCode::OK);
    let open_body = axum::body::to_bytes(open_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let opened: serde_json::Value = serde_json::from_slice(&open_body).unwrap();
    let page_id = opened["engine_page"]["page_id"].as_str().unwrap();

    let after_open_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/summary")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(after_open_response.status(), StatusCode::OK);
    let after_open_body = axum::body::to_bytes(after_open_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let after_open: serde_json::Value = serde_json::from_slice(&after_open_body).unwrap();
    assert_eq!(after_open["sessions"]["active_sessions"], 1);
    assert_eq!(after_open["sessions"]["principal_sessions"], 1);
    assert_eq!(
        after_open["sessions"]["heartbeat"]["route"],
        "/api/apps/browser/pages/:page_id/heartbeat"
    );

    let heartbeat_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/heartbeat"))
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(heartbeat_response.status(), StatusCode::OK);

    let close_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close_response.status(), StatusCode::OK);

    let after_close_response = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/summary")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(after_close_response.status(), StatusCode::OK);
    let after_close_body = axum::body::to_bytes(after_close_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let after_close: serde_json::Value = serde_json::from_slice(&after_close_body).unwrap();
    assert_eq!(after_close["sessions"]["active_sessions"], 0);
    assert_eq!(after_close["sessions"]["principal_sessions"], 0);
}

#[tokio::test]
async fn test_browser_session_capacity_tracks_multiple_pages_independently() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let first_open_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://ela.city/","reason":"multi-page Browser accounting regression","viewport":{"width":960,"height":540},"display_mode":"diagnostic_frame"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_open_response.status(), StatusCode::OK);
    let first_open_body = axum::body::to_bytes(first_open_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_opened: serde_json::Value = serde_json::from_slice(&first_open_body).unwrap();
    let first_page_id = first_opened["engine_page"]["page_id"]
        .as_str()
        .unwrap()
        .to_string();

    let second_open_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","reason":"second independent Browser page","viewport":{"width":960,"height":540},"display_mode":"diagnostic_frame"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_open_response.status(), StatusCode::OK);
    let second_open_body = axum::body::to_bytes(second_open_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let second_opened: serde_json::Value = serde_json::from_slice(&second_open_body).unwrap();
    let second_page_id = second_opened["engine_page"]["page_id"]
        .as_str()
        .unwrap()
        .to_string();

    assert_ne!(
        first_page_id, second_page_id,
        "separate Browser launches must not collapse into one shared page"
    );

    let after_open_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/summary")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(after_open_response.status(), StatusCode::OK);
    let after_open_body = axum::body::to_bytes(after_open_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let after_open: serde_json::Value = serde_json::from_slice(&after_open_body).unwrap();
    assert_eq!(
        after_open["sessions"]["schema"],
        "elastos.browser.session-capacity/v1"
    );
    assert_eq!(after_open["sessions"]["active_sessions"], 2);
    assert_eq!(after_open["sessions"]["launching_sessions"], 0);
    assert_eq!(after_open["sessions"]["total_sessions"], 2);
    assert_eq!(after_open["sessions"]["principal_sessions"], 2);
    assert_eq!(after_open["sessions"]["capacity_available"], true);

    for page_id in [&first_page_id, &second_page_id] {
        let heartbeat_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/apps/browser/pages/{page_id}/heartbeat"))
                    .header("x-elastos-home-token", token.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(heartbeat_response.status(), StatusCode::OK);
        let heartbeat_body = axum::body::to_bytes(heartbeat_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let heartbeat: serde_json::Value = serde_json::from_slice(&heartbeat_body).unwrap();
        assert_eq!(heartbeat["schema"], "elastos.browser.page-heartbeat/v1");
        assert_eq!(heartbeat["page_id"], *page_id);
    }

    let first_close_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{first_page_id}/close"))
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_close_response.status(), StatusCode::OK);

    let after_first_close_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/summary")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(after_first_close_response.status(), StatusCode::OK);
    let after_first_close_body =
        axum::body::to_bytes(after_first_close_response.into_body(), usize::MAX)
            .await
            .unwrap();
    let after_first_close: serde_json::Value =
        serde_json::from_slice(&after_first_close_body).unwrap();
    assert_eq!(after_first_close["sessions"]["active_sessions"], 1);
    assert_eq!(after_first_close["sessions"]["total_sessions"], 1);
    assert_eq!(after_first_close["sessions"]["principal_sessions"], 1);

    let second_close_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{second_page_id}/close"))
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_close_response.status(), StatusCode::OK);

    let after_all_close_response = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/summary")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(after_all_close_response.status(), StatusCode::OK);
    let after_all_close_body =
        axum::body::to_bytes(after_all_close_response.into_body(), usize::MAX)
            .await
            .unwrap();
    let after_all_close: serde_json::Value = serde_json::from_slice(&after_all_close_body).unwrap();
    assert_eq!(after_all_close["sessions"]["active_sessions"], 0);
    assert_eq!(after_all_close["sessions"]["total_sessions"], 0);
    assert_eq!(after_all_close["sessions"]["principal_sessions"], 0);
}

#[tokio::test]
async fn test_browser_open_fails_closed_when_display_session_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","reason":"open browser page","display_mode":"webrtc_remote_display"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("webrtc_remote_display"));
}

#[tokio::test]
async fn test_browser_personal_sign_queues_wallet_inbox_approval() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);
    let address = "0x1111111111111111111111111111111111111111";
    let account_id = format!("wallet:eip155:1:{address}");
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::new(vec![json!({
            "account_id": account_id,
            "principal_id": authority.principal_id,
            "proof_binding_id": "proof:wallet:managed:eip155:1:0x1111111111111111111111111111111111111111",
            "chain_namespace": "eip155:1",
            "address": address,
            "proof_type": "managed_evm",
            "signing_available": true,
            "label": "Spending",
            "linked_at": crate::auth::now_ts()
        })]),
        approvals: TokioMutex::default(),
        defaults: TokioMutex::default(),
    };
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/wallet/request-signature")
                .header("origin", "https://glidefinance.io")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{
                        "method":"personal_sign",
                        "params":["Sign into Glide","{address}"],
                        "account_id":"wallet:eip155:1:{address}",
                        "chain_namespace":"eip155:1",
                        "address":"{address}",
                        "page_url":"https://glidefinance.io/",
                        "origin":"https://glidefinance.io"
                    }}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("https://glidefinance.io")
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["schema"],
        "elastos.browser.wallet-approval-result/v1"
    );
    assert_eq!(
        payload["approval_request"]["intent"],
        "browser_personal_sign"
    );
    assert_eq!(
        payload["approval_request"]["capsule_id"],
        BROWSER_CAPSULE_ID
    );

    let inbox = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", inbox_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(inbox.status(), StatusCode::OK);
    let inbox_body = axum::body::to_bytes(inbox.into_body(), usize::MAX)
        .await
        .unwrap();
    let inbox_json: serde_json::Value = serde_json::from_slice(&inbox_body).unwrap();
    assert_eq!(inbox_json["notifications"]["attention_count"], 1);
    assert_eq!(
        inbox_json["notifications"]["entries"][0]["title"],
        "Browser signature request"
    );
}

#[tokio::test]
async fn test_browser_typed_data_sign_queues_wallet_inbox_approval() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);
    let address = "0x1111111111111111111111111111111111111111";
    let account_id = format!("wallet:eip155:20:{address}");
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::new(vec![json!({
            "account_id": account_id,
            "principal_id": authority.principal_id,
            "proof_binding_id": "proof:wallet:managed:eip155:20:0x1111111111111111111111111111111111111111",
            "chain_namespace": "eip155:20",
            "address": address,
            "proof_type": "managed_evm",
            "signing_available": true,
            "label": "Spending",
            "linked_at": crate::auth::now_ts()
        })]),
        approvals: TokioMutex::default(),
        defaults: TokioMutex::default(),
    };
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);
    let typed_data = json!({
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" },
                { "name": "chainId", "type": "uint256" }
            ],
            "Message": [
                { "name": "contents", "type": "string" }
            ]
        },
        "primaryType": "Message",
        "domain": { "name": "ElastOS Browser", "chainId": 20 },
        "message": { "contents": "Connect wallet" }
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/wallet/request-signature")
                .header("origin", "https://ela.city")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "method": "eth_signTypedData_v4",
                        "params": [address, typed_data],
                        "account_id": format!("wallet:eip155:20:{address}"),
                        "chain_namespace": "eip155:20",
                        "address": address,
                        "page_url": "https://ela.city/home",
                        "origin": "https://ela.city"
                    })
                    .to_string(),
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
        payload["approval_request"]["intent"],
        "browser_typed_data_sign"
    );
    assert_eq!(
        payload["approval_request"]["resource"],
        "elastos://wallet/eip155:20/sign/browser_typed_data_sign"
    );

    let inbox = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", inbox_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(inbox.status(), StatusCode::OK);
    let inbox_body = axum::body::to_bytes(inbox.into_body(), usize::MAX)
        .await
        .unwrap();
    let inbox_json: serde_json::Value = serde_json::from_slice(&inbox_body).unwrap();
    assert_eq!(inbox_json["notifications"]["attention_count"], 1);
    assert_eq!(
        inbox_json["notifications"]["entries"][0]["title"],
        "Browser typed data signature request"
    );
}

#[tokio::test]
async fn test_browser_wallet_approval_routes_allow_remote_page_preflight() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let response = app
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/api/apps/browser/wallet/request-signature")
                .header("origin", "https://ela.city")
                .header("access-control-request-method", "POST")
                .header(
                    "access-control-request-headers",
                    "content-type,x-elastos-home-token",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("https://ela.city")
    );
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-methods")
            .and_then(|value| value.to_str().ok()),
        Some("GET, POST, OPTIONS")
    );
    assert!(response
        .headers()
        .get("access-control-allow-headers")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("x-elastos-home-token")));
}

#[tokio::test]
async fn test_browser_chain_reads_route_through_chain_provider_without_inbox_approval() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);
    let address = "0x1111111111111111111111111111111111111111";
    let account_id = format!("wallet:eip155:20:{address}");
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::new(vec![json!({
            "account_id": account_id,
            "principal_id": authority.principal_id,
            "proof_binding_id": "proof:wallet:managed:eip155:20:0x1111111111111111111111111111111111111111",
            "chain_namespace": "eip155:20",
            "address": address,
            "proof_type": "managed_evm",
            "signing_available": true,
            "label": "Spending",
            "linked_at": crate::auth::now_ts()
        })]),
        approvals: TokioMutex::default(),
        defaults: TokioMutex::default(),
    };
    let app =
        gateway_router(wallet_chain_test_state_with_wallet_provider(dir.path(), provider).await);

    let block_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/wallet/read")
                .header("origin", "https://ela.city")
                .header("x-elastos-home-token", browser_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "method": "eth_blockNumber",
                        "params": [],
                        "chain_namespace": "eip155:20",
                        "address": address,
                        "page_url": "https://ela.city/",
                        "origin": "https://ela.city"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(block_response.status(), StatusCode::OK);
    assert_eq!(
        block_response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("https://ela.city")
    );
    let block_body = axum::body::to_bytes(block_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let block_payload: serde_json::Value = serde_json::from_slice(&block_body).unwrap();
    assert_eq!(
        block_payload["schema"],
        "elastos.browser.wallet-read-result/v1"
    );
    assert_eq!(block_payload["method"], "eth_blockNumber");
    assert_eq!(block_payload["result"], "0x2a");
    assert_eq!(block_payload["requires_approval"], false);
    assert_eq!(block_payload["authority"], "runtime_chain_provider");

    let balance_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/wallet/read")
                .header("x-elastos-home-token", browser_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "method": "eth_getBalance",
                        "params": [address, "latest"],
                        "chain_namespace": "eip155:20",
                        "address": address,
                        "page_url": "https://ela.city/",
                        "origin": "https://ela.city"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(balance_response.status(), StatusCode::OK);
    let balance_body = axum::body::to_bytes(balance_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let balance_payload: serde_json::Value = serde_json::from_slice(&balance_body).unwrap();
    assert_eq!(balance_payload["method"], "eth_getBalance");
    assert_eq!(balance_payload["result"], "0xde0b6b3a7640000");
    assert_eq!(balance_payload["requires_approval"], false);

    let call_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/wallet/read")
                .header("x-elastos-home-token", browser_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "method": "eth_call",
                        "params": [{
                            "to": "0x2222222222222222222222222222222222222222",
                            "data": "0x70a082310000000000000000000000001111111111111111111111111111111111111111"
                        }, "latest"],
                        "chain_namespace": "eip155:20",
                        "address": address,
                        "page_url": "https://glidefinance.io/",
                        "origin": "https://glidefinance.io"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(call_response.status(), StatusCode::OK);
    let call_body = axum::body::to_bytes(call_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let call_payload: serde_json::Value = serde_json::from_slice(&call_body).unwrap();
    assert_eq!(call_payload["method"], "eth_call");
    assert_eq!(
        call_payload["result"],
        "0x0000000000000000000000000000000000000000000000000000000000000042"
    );
    assert_eq!(call_payload["requires_approval"], false);

    let estimate_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/wallet/read")
                .header("x-elastos-home-token", browser_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "method": "eth_estimateGas",
                        "params": [{
                            "from": address,
                            "to": "0x3333333333333333333333333333333333333333",
                            "value": "0x1",
                            "data": "0xa9059cbb0000000000000000000000004444444444444444444444444444444444444444"
                        }],
                        "chain_namespace": "eip155:20",
                        "address": address,
                        "page_url": "https://ela.city/",
                        "origin": "https://ela.city"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(estimate_response.status(), StatusCode::OK);
    let estimate_body = axum::body::to_bytes(estimate_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let estimate_payload: serde_json::Value = serde_json::from_slice(&estimate_body).unwrap();
    assert_eq!(estimate_payload["method"], "eth_estimateGas");
    assert_eq!(estimate_payload["result"], "0x5208");
    assert_eq!(estimate_payload["requires_approval"], false);

    let extra_reads = vec![
        (
            "eth_getTransactionCount",
            json!([address, "pending"]),
            json!("0x7"),
        ),
        ("eth_gasPrice", json!([]), json!("0x3b9aca00")),
        (
            "eth_feeHistory",
            json!(["0x1", "latest", [1.0]]),
            json!({
                "oldestBlock": "0x1",
                "baseFeePerGas": ["0x3b9aca00", "0x3b9aca01"],
                "gasUsedRatio": [0.5],
                "reward": [["0x1"]]
            }),
        ),
        (
            "eth_getCode",
            json!(["0x2222222222222222222222222222222222222222", "latest"]),
            json!("0x60016001"),
        ),
        (
            "eth_getLogs",
            json!([{
                "fromBlock": "0x1",
                "toBlock": "latest",
                "address": "0x2222222222222222222222222222222222222222",
                "topics": []
            }]),
            json!([{
                "address": "0x2222222222222222222222222222222222222222",
                "blockNumber": "0x2a",
                "data": "0x",
                "topics": []
            }]),
        ),
    ];
    for (method, params, expected) in extra_reads {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/browser/wallet/read")
                    .header("x-elastos-home-token", browser_token.clone())
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "method": method,
                            "params": params,
                            "chain_namespace": "eip155:20",
                            "address": address,
                            "page_url": "https://ela.city/",
                            "origin": "https://ela.city"
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
        assert_eq!(
            status,
            StatusCode::OK,
            "{method} failed: {}",
            String::from_utf8_lossy(&body)
        );
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["method"], method);
        assert_eq!(payload["result"], expected);
        assert_eq!(payload["requires_approval"], false);
    }

    let inbox = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", inbox_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(inbox.status(), StatusCode::OK);
    let inbox_body = axum::body::to_bytes(inbox.into_body(), usize::MAX)
        .await
        .unwrap();
    let inbox_json: serde_json::Value = serde_json::from_slice(&inbox_body).unwrap();
    assert_eq!(inbox_json["notifications"]["attention_count"], 0);

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let read_events: Vec<_> = auth_state
        .audit
        .iter()
        .filter(|event| {
            event.capsule_id.as_deref() == Some(BROWSER_CAPSULE_ID)
                && event.event_type.starts_with("browser.chain_read.")
        })
        .collect();
    assert_eq!(
        read_events.len(),
        18,
        "each Browser chain read must emit requested + completed audit events"
    );
    assert!(
        read_events
            .iter()
            .any(|event| event.event_type == "browser.chain_read.requested"
                && event.result == "requested"
                && event.reason.contains("decision=standing_read_policy")),
        "read-only Browser chain calls may use standing policy, but must be audited"
    );
    assert!(
        read_events
            .iter()
            .any(|event| event.event_type == "browser.chain_read.completed"
                && event.result == "allowed"
                && event
                    .reason
                    .contains("decision=provider_mediated_typed_read")),
        "successful Browser chain reads must record provider-mediated execution"
    );
}

#[tokio::test]
async fn test_browser_eth_send_transaction_queues_wallet_inbox_approval() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);
    let address = "0x1111111111111111111111111111111111111111";
    let account_id = format!("wallet:eip155:20:{address}");
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::new(vec![json!({
            "account_id": account_id,
            "principal_id": authority.principal_id,
            "proof_binding_id": "proof:wallet:managed:eip155:20:0x1111111111111111111111111111111111111111",
            "chain_namespace": "eip155:20",
            "address": address,
            "proof_type": "managed_evm",
            "label": "Spending",
            "linked_at": crate::auth::now_ts()
        })]),
        approvals: TokioMutex::default(),
        defaults: TokioMutex::default(),
    };
    let app =
        gateway_router(wallet_chain_test_state_with_wallet_provider(dir.path(), provider).await);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/wallet/request-transaction")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{
                        "method":"eth_sendTransaction",
                        "params":[{{"from":"{address}","to":"0x2222222222222222222222222222222222222222","value":"0x1","data":"0x"}}],
                        "account_id":"wallet:eip155:20:{address}",
                        "chain_namespace":"eip155:20",
                        "address":"{address}",
                        "page_url":"https://glidefinance.io/",
                        "origin":"https://glidefinance.io"
                    }}"#
                )))
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
        payload["schema"],
        "elastos.browser.wallet-approval-result/v1"
    );
    assert_eq!(payload["approval_request"]["intent"], "transaction_intent");
    assert_eq!(
        payload["approval_request"]["resource"],
        "elastos://chain/esc-mainnet/broadcast_transaction"
    );

    let inbox = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", inbox_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(inbox.status(), StatusCode::OK);
    let inbox_body = axum::body::to_bytes(inbox.into_body(), usize::MAX)
        .await
        .unwrap();
    let inbox_json: serde_json::Value = serde_json::from_slice(&inbox_body).unwrap();
    assert_eq!(inbox_json["notifications"]["attention_count"], 1);
    assert_eq!(
        inbox_json["notifications"]["entries"][0]["title"],
        "Transaction approval request"
    );
}

#[tokio::test]
async fn test_browser_eth_send_transaction_allows_external_connector_approval() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let address = "0x3333333333333333333333333333333333333333";
    let account_id = format!("wallet:eip155:20:{address}");
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::new(vec![json!({
            "account_id": account_id,
            "principal_id": authority.principal_id,
            "proof_binding_id": "proof:wallet:siwe:eip155:20:0x3333333333333333333333333333333333333333",
            "chain_namespace": "eip155:20",
            "address": address,
            "proof_type": "siwe",
            "connector_id": "wallet-metamask",
            "label": "Family",
            "linked_at": crate::auth::now_ts()
        })]),
        approvals: TokioMutex::default(),
        defaults: TokioMutex::default(),
    };
    let app =
        gateway_router(wallet_chain_test_state_with_wallet_provider(dir.path(), provider).await);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/wallet/request-transaction")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{
                        "method":"eth_sendTransaction",
                        "params":[{{"from":"{address}","to":"0x2222222222222222222222222222222222222222","value":"0x1","data":"0x"}}],
                        "account_id":"wallet:eip155:20:{address}",
                        "chain_namespace":"eip155:20",
                        "address":"{address}",
                        "page_url":"https://glidefinance.io/",
                        "origin":"https://glidefinance.io"
                    }}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["approval_request"]["intent"], "transaction_intent");
    assert_eq!(
        payload["approval_request"]["connector_id"],
        "wallet-metamask"
    );
}

#[tokio::test]
async fn test_browser_wallet_approval_status_returns_completed_signature() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![json!({
            "request_id": "wallet-approval:browser",
            "status": "completed",
            "intent": "browser_personal_sign",
            "capsule_id": BROWSER_CAPSULE_ID,
            "resource": "elastos://wallet/eip155:1/sign/browser_personal_sign",
            "reason": "Browser page requests personal_sign",
            "account_id": "wallet:eip155:1:0x1111111111111111111111111111111111111111",
            "chain_namespace": "eip155:1",
            "address": "0x1111111111111111111111111111111111111111",
            "proof_type": "managed_evm",
            "payload_hash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "principal_id": authority.principal_id,
            "created_at": 10,
            "expires_at": 20,
            "completed_at": 12,
            "signed_result": {
                "schema": "elastos.browser.personal-sign-result/v1",
                "request_id": "wallet-approval:browser",
                "method": "personal_sign",
                "signature": "0xsigned",
                "signer": "0x1111111111111111111111111111111111111111"
            }
        })]),
        defaults: TokioMutex::default(),
    };
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/wallet/approvals/wallet-approval%3Abrowser")
                .header("x-elastos-home-token", browser_token)
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
    assert_eq!(
        payload["schema"],
        "elastos.browser.wallet-approval-status/v1"
    );
    assert_eq!(payload["status"], "completed");
    assert_eq!(payload["signature"], "0xsigned");
}

#[tokio::test]
async fn test_browser_completed_transaction_approval_broadcasts_through_chain_provider() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![json!({
            "request_id": "wallet-approval:browser-tx",
            "status": "completed",
            "intent": "transaction_intent",
            "capsule_id": BROWSER_CAPSULE_ID,
            "resource": "elastos://chain/esc-mainnet/broadcast_transaction",
            "reason": "Browser page requests eth_sendTransaction on esc-mainnet",
            "account_id": "wallet:eip155:20:0x1111111111111111111111111111111111111111",
            "chain_namespace": "eip155:20",
            "address": "0x1111111111111111111111111111111111111111",
            "proof_type": "managed_evm",
            "payload_hash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "principal_id": authority.principal_id,
            "created_at": 10,
            "expires_at": 20,
            "completed_at": 12,
            "signed_result": {
                "schema": "elastos.wallet.signed-transaction-result/v1",
                "request_id": "wallet-approval:browser-tx",
                "method": "eth_sendTransaction",
                "signed_transaction": "0x02f8",
                "chain_namespace": "eip155:20"
            }
        })]),
        defaults: TokioMutex::default(),
    };
    let app =
        gateway_router(wallet_chain_test_state_with_wallet_provider(dir.path(), provider).await);

    let status_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/wallet/approvals/wallet-approval%3Abrowser-tx")
                .header("x-elastos-home-token", browser_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status_response.status(), StatusCode::OK);
    let status_body = axum::body::to_bytes(status_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
    assert_eq!(status_json["status"], "completed");
    assert_eq!(status_json["signed_transaction"], "0x02f8");
    assert!(status_json["transaction_hash"].is_null());

    let broadcast_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"request_id":"wallet-approval:browser-tx"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(broadcast_response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(broadcast_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["schema"],
        "elastos.browser.transaction-broadcast/v1"
    );
    assert_eq!(
        payload["transaction_hash"],
        "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(payload["recorded"], true);

    let recorded_status = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/wallet/approvals/wallet-approval%3Abrowser-tx")
                .header("x-elastos-home-token", browser_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(recorded_status.status(), StatusCode::OK);
    let recorded_body = axum::body::to_bytes(recorded_status.into_body(), usize::MAX)
        .await
        .unwrap();
    let recorded_json: serde_json::Value = serde_json::from_slice(&recorded_body).unwrap();
    assert_eq!(
        recorded_json["transaction_hash"],
        "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );

    let second_broadcast_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"request_id":"wallet-approval:browser-tx"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_broadcast_response.status(), StatusCode::OK);
    let second_body = axum::body::to_bytes(second_broadcast_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let second_payload: serde_json::Value = serde_json::from_slice(&second_body).unwrap();
    assert_eq!(
        second_payload["transaction_hash"],
        "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(second_payload["already_recorded"], true);
}

#[tokio::test]
async fn test_browser_completed_external_transaction_returns_hash_without_broadcast() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let tx_hash = "0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![json!({
            "request_id": "wallet-approval:browser-external-tx",
            "status": "completed",
            "intent": "transaction_intent",
            "capsule_id": BROWSER_CAPSULE_ID,
            "resource": "elastos://chain/esc-mainnet/broadcast_transaction",
            "reason": "Browser page requests eth_sendTransaction on esc-mainnet",
            "account_id": "wallet:eip155:20:0x3333333333333333333333333333333333333333",
            "chain_namespace": "eip155:20",
            "address": "0x3333333333333333333333333333333333333333",
            "proof_type": "siwe",
            "connector_id": "wallet-metamask",
            "payload_hash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "principal_id": authority.principal_id,
            "created_at": 10,
            "expires_at": 20,
            "completed_at": 12,
            "signed_result": {
                "schema": "elastos.wallet.external-transaction-result/v1",
                "request_id": "wallet-approval:browser-external-tx",
                "method": "eth_sendTransaction",
                "transaction_hash": tx_hash,
                "chain_namespace": "eip155:20"
            }
        })]),
        defaults: TokioMutex::default(),
    };
    let app =
        gateway_router(wallet_chain_test_state_with_wallet_provider(dir.path(), provider).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/wallet/approvals/wallet-approval%3Abrowser-external-tx")
                .header("x-elastos-home-token", browser_token)
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
    assert_eq!(payload["status"], "completed");
    assert_eq!(payload["transaction_hash"], tx_hash);
    assert!(payload.get("signed_transaction").is_none());
}

#[tokio::test]
async fn test_browser_page_screenshot_and_input_are_runtime_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let screenshot = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/pages/page%3Amock-browser-engine/screenshot")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(screenshot.status(), StatusCode::OK);
    assert_eq!(
        screenshot.headers().get(CONTENT_TYPE).unwrap(),
        HeaderValue::from_static("image/png")
    );
    let image = axum::body::to_bytes(screenshot.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&image[..], b"mock-png");

    let status = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/pages/page%3Amock-browser-engine/status")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    let body = axum::body::to_bytes(status.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["schema"], "elastos.browser.page-status/v1");
    assert_eq!(payload["page_id"], "page:mock-browser-engine");
    assert_eq!(payload["direct_network"], false);
    assert_eq!(payload["webrtc_connection_state"], "connected");

    let frame = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/browser/pages/page%3Amock-browser-engine/frame?since=1&wait_ms=25")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(frame.status(), StatusCode::OK);
    let body = axum::body::to_bytes(frame.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["schema"], "elastos.browser.frame/v1");
    assert_eq!(payload["page_id"], "page:mock-browser-engine");
    assert_eq!(payload["seq"], 2);
    assert!(payload["base64"].as_str().unwrap().len() > 8);

    let input = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/pages/page%3Amock-browser-engine/input")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"event":{"type":"click","x":12,"y":34}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(input.status(), StatusCode::OK);
    let body = axum::body::to_bytes(input.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["schema"], "elastos.browser.input-result/v1");
    assert_eq!(payload["page_id"], "page:mock-browser-engine");
    assert!(payload["screenshot"].as_str().unwrap().len() > 8);

    let close = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/pages/page%3Amock-browser-engine/close")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close.status(), StatusCode::OK);
    let body = axum::body::to_bytes(close.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["schema"], "elastos.browser.close-result/v1");
    assert_eq!(payload["page_id"], "page:mock-browser-engine");
    assert_eq!(payload["closed"], true);
}

#[tokio::test]
async fn test_browser_webrtc_signal_is_runtime_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/pages/page%3Amock-browser-engine/webrtc")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"type":"offer","sdp":"v=0\r\ns=ElastOS Browser Test\r\n"}"#,
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
    assert_eq!(payload["schema"], "elastos.browser.webrtc-answer/v1");
    assert_eq!(payload["page_id"], "page:mock-browser-engine");
    assert_eq!(payload["type"], "answer");
    assert!(payload["sdp"].as_str().unwrap().starts_with("v=0"));
}

#[tokio::test]
async fn test_browser_webrtc_offer_rejects_embedded_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/pages/page%3Amock-browser-engine/webrtc")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"type":"offer","sdp":"v=0\r\na=candidate:1 1 udp 2113937151 host.local 56929 typ host generation 0 network-cost 999\r\n"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains("ICE candidates through candidate messages"));
}

#[tokio::test]
async fn test_browser_webrtc_answer_signal_is_runtime_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/pages/page%3Amock-browser-engine/webrtc")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"type":"answer","sdp":"v=0\r\ns=ElastOS Browser Test\r\n"}"#,
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
    assert_eq!(payload["schema"], "elastos.browser.webrtc-signal-ack/v1");
    assert_eq!(payload["page_id"], "page:mock-browser-engine");
    assert_eq!(payload["type"], "answer");
}

#[tokio::test]
async fn test_browser_webrtc_candidate_signal_is_runtime_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/pages/page%3Amock-browser-engine/webrtc")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"type":"candidate","candidate":{"candidate":"candidate:1 1 udp 2113937151 host.local 56929 typ host generation 0 network-cost 999","sdpMid":"0","sdpMLineIndex":0}}"#,
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
    assert_eq!(payload["schema"], "elastos.browser.webrtc-signal-ack/v1");
    assert_eq!(payload["type"], "candidate");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/pages/page%3Amock-browser-engine/webrtc")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"type":"end_of_candidates"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["schema"], "elastos.browser.webrtc-signal-ack/v1");
    assert_eq!(payload["type"], "end_of_candidates");
}

#[tokio::test]
async fn test_browser_webrtc_signal_preserves_provider_error() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/pages/page%3Amock-browser-engine/webrtc")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"type":"offer","sdp":"v=0\r\ns=simulate-provider-error\r\n"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains("browser page not found"));
    assert!(!body.contains("invalid WebRTC answer schema"));
}

#[tokio::test]
#[cfg(unix)]
async fn test_browser_open_runtime_stream_socket_accepts_and_closes_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","reason":"open browser page","display_mode":"diagnostic_frame"}"#,
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

    let stream_id = payload["stream_session"]["stream_id"].as_str().unwrap();
    let socket_path = browser_runtime_stream_socket_path(dir.path(), stream_id).unwrap();
    let mut stream = UnixStream::connect(&socket_path).await.unwrap();
    let mut byte = [0_u8; 1];
    let read = tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut byte))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read, 0);
}

#[tokio::test]
#[cfg(unix)]
async fn test_browser_open_runtime_stream_relays_to_exit_ipc_without_host_network() {
    let dir = tempfile::tempdir().unwrap();
    let relay_path = dir.path().join("mock-exit-relay.sock");
    let relay_listener = UnixListener::bind(&relay_path).unwrap();
    let expected_stream_id = mock_attached_stream_id(dir.path());
    let relay_task = tokio::spawn(async move {
        let (mut relay, _addr) = relay_listener.accept().await.unwrap();
        let mut header = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            relay.read_exact(&mut byte).await.unwrap();
            if byte[0] == b'\n' {
                break;
            }
            header.push(byte[0]);
        }
        let header: serde_json::Value = serde_json::from_slice(&header).unwrap();
        assert_eq!(header["schema"], "elastos.exit.relay-open/v1");
        assert_eq!(header["stream_id"], expected_stream_id);
        assert_eq!(header["target"], "tls://glidefinance.io:443");
        assert_eq!(header["host"], "glidefinance.io");
        let mut request = [0_u8; 4];
        relay.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"ping");
        tokio::io::AsyncWriteExt::write_all(&mut relay, b"pong")
            .await
            .unwrap();
    });

    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(
        browser_engine_attached_test_state_with_relay(
            dir.path(),
            Some(relay_path.to_string_lossy().to_string()),
        )
        .await,
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","reason":"open browser page","display_mode":"diagnostic_frame"}"#,
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
    assert!(payload["stream_session"].get("adapter_ipc").is_none());
    assert!(payload["stream_session"].get("relay_ipc").is_none());

    let stream_id = payload["stream_session"]["stream_id"].as_str().unwrap();
    let socket_path = browser_runtime_stream_socket_path(dir.path(), stream_id).unwrap();
    let mut stream = UnixStream::connect(&socket_path).await.unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut stream, b"ping")
        .await
        .unwrap();
    let mut response = [0_u8; 4];
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        stream.read_exact(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(&response, b"pong");
    relay_task.await.unwrap();
}

#[tokio::test]
async fn test_browser_net_provider_fails_closed_without_adapter_provider() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let system_token = app_token_for_authority(dir.path(), SYSTEM_CAPSULE_ID, &authority);
    seed_test_browser_capsules(dir.path());
    let app = gateway_router(GatewayState {
        provider_registry: Some(Arc::new(ProviderRegistry::new())),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: dir.path().to_path_buf(),
        data_dir: dir.path().to_path_buf(),
        audit_log: Arc::new(std::sync::OnceLock::new()),
        spend_policy: None,
    });

    let forbidden = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/net/http")
                .header("x-elastos-home-token", system_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"url":"https://glidefinance.io/"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let unavailable = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/net/http")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"url":"https://glidefinance.io/"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(unavailable.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("net provider unavailable"));
}

#[tokio::test]
async fn test_browser_net_provider_error_status_maps_to_fail_closed_http() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(net_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/net/http")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"schema":"elastos.browser.net-request/v1","url":"https://glidefinance.io/","method":"GET"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("exit provider unavailable"));
    assert!(message.contains("No Browser Exit provider is configured"));
}

#[tokio::test]
async fn test_browser_net_http_hands_validated_request_to_internal_exit_provider() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(net_exit_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/net/http")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"schema":"elastos.browser.net-request/v1","url":"https://glidefinance.io/","method":"GET","reason":"open browser address"}"#,
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
    assert_eq!(payload["status"], "ok");
    assert_eq!(
        payload["data"]["schema"],
        "elastos.exit.http-fetch.result/v1"
    );
    assert_eq!(payload["data"]["backend"], "mock-exit");
    assert_eq!(payload["data"]["url"], "https://glidefinance.io/");
    assert_eq!(payload["data"]["body_text"], "mock exit body");
}

#[tokio::test]
async fn test_browser_net_stream_hands_validated_request_to_internal_exit_provider() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(net_exit_test_state(dir.path()).await);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/net/stream")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"target":"tls://glidefinance.io:443","reason":"open browser stream"}"#,
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
    assert_eq!(payload["status"], "ok");
    assert_eq!(payload["data"]["schema"], "elastos.exit.stream-session/v1");
    assert_eq!(payload["data"]["backend"], "mock-exit");
    assert_eq!(payload["data"]["target"], "tls://glidefinance.io:443");
    assert_eq!(payload["data"]["engine_owns_tls"], true);
    assert_eq!(payload["data"]["byte_transport"], "not_attached");
}
