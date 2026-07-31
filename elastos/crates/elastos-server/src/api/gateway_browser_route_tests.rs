use super::*;
use crate::api::browser_engine_protocol::{
    BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA, BROWSER_ENGINE_PROTOCOL_VERSION,
};
use crate::api::gateway::gateway_browser::{
    browser_engine_cleanup_obligation_count, browser_launch_reconciliation_obligation_count,
    browser_lifecycle_hash, browser_page_session_count, browser_principal_has_live_sessions,
    browser_stream_cleanup_obligation_count, complete_browser_launch, reserve_browser_launch,
    BrowserLaunchEffect, BrowserLaunchLifecycle,
};

async fn open_mock_browser_page_result(
    app: axum::Router,
    token: &str,
    reason: &str,
) -> serde_json::Value {
    let open = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "url": "glidefinance.io",
                        "reason": reason,
                        "viewport": {"width": 900, "height": 520},
                        "display_mode": "webrtc_remote_display",
                        "guarantee_level": "operator_rbi"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = open.status();
    let body = axum::body::to_bytes(open.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "Browser open failed: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).unwrap()
}

async fn open_mock_browser_page(app: axum::Router, token: &str, reason: &str) -> String {
    let opened = open_mock_browser_page_result(app, token, reason).await;
    opened["engine_page"]["page_id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn browser_cleanup_id(opened: &serde_json::Value) -> &str {
    opened["runtime_cleanup"]["id"]
        .as_str()
        .expect("Browser open cleanup identifier")
}

fn browser_close_body(cleanup_id: &str) -> Body {
    Body::from(
        json!({
            "schema": "elastos.browser.close-request/v2",
            "cleanup_id": cleanup_id,
        })
        .to_string(),
    )
}

#[tokio::test]
async fn test_browser_app_summary_declares_fail_closed_engine_adapter() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(test_state(dir.path()));

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
    assert_eq!(payload["sessions"]["max_active_sessions"], 4);
    assert_eq!(payload["sessions"]["max_sessions_per_principal"], 4);
    assert_eq!(payload["sessions"]["capacity_available"], true);
    assert_eq!(
        payload["sessions"]["lifecycle"]["schema"],
        "elastos.browser.lifecycle-status/v1"
    );
    assert_eq!(payload["sessions"]["lifecycle"]["owner"], "runtime_gateway");
    assert!(payload["sessions"]["lifecycle"]["phases"]
        .as_array()
        .unwrap()
        .iter()
        .any(|phase| phase.as_str() == Some("STARTING_VM")));
    assert!(payload["sessions"]["lifecycle"]["sessions"]
        .as_array()
        .unwrap()
        .is_empty());
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
            test_browser_request("localhost:61180", "null")
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
async fn test_browser_app_summary_reports_remote_carrier_exit_policy_without_authority_leaks() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_remote_carrier_exit_test_state(dir.path()).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
    let exit = &payload["net"]["exit_provider"];
    assert_eq!(exit["provider"], "elastos://exit/*");
    assert_eq!(exit["direct_network"], false);
    assert_eq!(exit["remote_carrier_exit_count"], 1);
    let remote_exits = exit["remote_carrier_exits"].as_array().unwrap();
    assert_eq!(remote_exits.len(), 1);
    assert_eq!(remote_exits[0]["id"], "mock-remote-carrier-exit");
    assert_eq!(
        remote_exits[0]["grant_id"],
        "operator-grant:mock-remote-carrier-exit:test"
    );
    assert_eq!(remote_exits[0]["transport"], "carrier_stream");
    assert_eq!(remote_exits[0]["allowed_for_principal"], true);
    assert_eq!(
        remote_exits[0].get("allowed_principals"),
        None,
        "Browser summary must not leak remote exit allowlists"
    );
    assert_eq!(
        remote_exits[0].pointer("/carrier/connect_ticket"),
        None,
        "Browser summary must not leak Carrier connect tickets"
    );
    assert_eq!(
        remote_exits[0].get("adapter_ipc"),
        None,
        "Browser summary must not leak host adapter sockets"
    );
    assert_eq!(
        remote_exits[0].get("relay_ipc"),
        None,
        "Browser summary must not leak host relay sockets"
    );
}

#[tokio::test]
async fn test_browser_app_summary_reports_registered_engine_adapter_status() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_test_state(dir.path()).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
    assert_eq!(payload["engine_adapter"]["adapter_count"], 2);
    assert_eq!(
        payload["engine_adapter"]["adapters"][0]["id"],
        "mock-browser-engine"
    );
    assert_eq!(
        payload["engine_adapter"]["adapters"][0]["supported_display_modes"][0],
        "webrtc_remote_display"
    );
    assert_eq!(
        payload["engine_adapter"]["adapters"][0]["backing_substrate"],
        "operator_rbi"
    );
    assert_eq!(
        payload["engine_adapter"]["adapters"][1]["id"],
        "mock-jetson-engine"
    );
    assert_eq!(payload["engine_adapter"]["adapters"][1]["default"], false);
    assert_eq!(payload["engine_adapter"]["byte_transport"], "adapter_ipc");
    assert_eq!(
        payload["engine_adapter"]["display_session_schema"],
        "elastos.browser.display-session/v1"
    );
    assert_eq!(
        payload["engine_adapter"]["supported_display_modes"][0],
        "webrtc_remote_display"
    );
    assert_eq!(
        payload["engine_adapter"]["supported_guarantee_levels"][0],
        "operator_rbi"
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
            test_browser_request("localhost:61180", "null")
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
async fn test_browser_open_rejects_invalid_adapter_inventory_before_reservation() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(malformed_browser_summary_test_state(dir.path()).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://invalid-adapter-inventory.invalid/","reason":"fail before reservation","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["message"]
        .as_str()
        .is_some_and(|message| message.contains("Adapter status authority is invalid")));
    assert_eq!(browser_page_session_count(dir.path()).await, 0);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(browser_stream_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(
        browser_launch_reconciliation_obligation_count(dir.path()).await,
        0
    );
    assert!(!dir.path().join("browser-lifecycle").exists());
    assert!(!dir.path().join("browser-streams").exists());
}

#[tokio::test]
async fn test_browser_open_fails_closed_without_attached_engine_transport() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_test_state(dir.path()).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","reason":"open browser page","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
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
async fn test_browser_open_requires_explicit_launch_contract() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","reason":"open browser page"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("display_mode") || message.contains("guarantee_level"));
}

#[tokio::test]
async fn test_browser_open_rejects_mismatched_launch_contract() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","display_mode":"native_surface","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("operator_rbi"));
}

#[tokio::test]
async fn test_browser_open_rejects_unsafe_remote_exit_id() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","remote_exit_id":"../server-exit","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("remote_exit_id"));
}

#[tokio::test]
async fn test_browser_open_rejects_unsafe_engine_adapter_id() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","adapter_id":"../mac-engine","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("adapter_id"));
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
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"javascript:alert(1)","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
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
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://whatismyip.com/","reason":"check exit IP","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
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
async fn test_browser_open_attaches_runtime_stream_for_remote_carrier_exit() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let app = gateway_router(
        browser_engine_remote_carrier_exit_test_state_with_close_calls(
            dir.path(),
            close_calls.clone(),
        )
        .await,
    );

    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","reason":"open browser page through remote exit","remote_exit_id":"mock-remote-carrier-exit","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
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
        payload["stream_session"]["schema"],
        "elastos.exit.remote-carrier-session/v1"
    );
    assert_eq!(
        payload["stream_session"]["byte_transport"],
        "carrier_stream"
    );
    assert_eq!(
        payload["stream_session"]["principal_id"].as_str(),
        Some(authority.principal_id.as_str())
    );
    assert_eq!(
        payload["stream_session"]["reason"].as_str(),
        Some("open browser page through remote exit")
    );
    assert_eq!(
        payload["stream_session"]["grant_id"].as_str(),
        Some("operator-grant:mock-remote-carrier-exit:test")
    );
    assert_eq!(
        payload["stream_session"]["backend"].as_str(),
        Some("mock-remote-carrier-exit")
    );
    assert_eq!(
        payload["stream_session"]["accounting"]["principal_id"].as_str(),
        Some(authority.principal_id.as_str())
    );
    assert!(payload["stream_session"].get("adapter_ipc").is_none());
    assert!(payload["stream_session"].get("relay_ipc").is_none());
    assert_eq!(
        payload["stream_session"].pointer("/carrier/connect_ticket"),
        None,
        "Browser response must not expose private Carrier route tickets"
    );
    let stream_id = payload["stream_session"]["stream_id"].as_str().unwrap();
    let socket_path = browser_runtime_stream_socket_path(dir.path(), stream_id).unwrap();
    assert!(socket_path.exists());
    let page_id = payload["engine_page"]["page_id"].as_str().unwrap();
    let cleanup_id = browser_cleanup_id(&payload);

    let close = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close.status(), StatusCode::OK);
    let calls = close_calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["op"], "close_stream");
    assert_eq!(calls[0]["stream_id"], stream_id);
    assert_eq!(calls[0]["principal_id"], authority.principal_id);
}

#[tokio::test]
async fn test_browser_close_stream_failure_retires_page_and_queues_stream_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let app = gateway_router(
        browser_engine_remote_carrier_exit_test_state_with_close_failures(
            dir.path(),
            close_calls.clone(),
            1,
        )
        .await,
    );

    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","reason":"open browser page through remote exit","remote_exit_id":"mock-remote-carrier-exit","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
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
    let page_id = payload["engine_page"]["page_id"].as_str().unwrap();
    let cleanup_id = browser_cleanup_id(&payload).to_string();

    let failed_close = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(&cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(failed_close.status(), StatusCode::SERVICE_UNAVAILABLE);
    let failed_body = axum::body::to_bytes(failed_close.into_body(), usize::MAX)
        .await
        .unwrap();
    let failed_message = String::from_utf8(failed_body.to_vec()).unwrap();
    assert!(failed_message.contains("simulated remote Carrier Exit close_stream failure"));
    {
        let calls = close_calls.lock().await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["stream_id"], stream_id);
    }

    let status_after_failed_close = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!("/api/apps/browser/pages/{page_id}/status"))
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status_after_failed_close.status(), StatusCode::NOT_FOUND);

    let retry_close = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retry_close.status(), StatusCode::BAD_REQUEST);
    {
        let calls = close_calls.lock().await;
        assert_eq!(calls.len(), 1);
    }

    let exact_retry = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(&cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(exact_retry.status(), StatusCode::OK);
    assert_eq!(close_calls.lock().await.len(), 2);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(browser_stream_cleanup_obligation_count(dir.path()).await, 0);
    assert!(!browser_principal_has_live_sessions(dir.path(), &authority.principal_id).await);
}

#[tokio::test]
async fn test_browser_open_failure_closes_remote_carrier_stream_reservation() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let app = gateway_router(
        rejecting_browser_engine_remote_carrier_exit_test_state_with_close_calls(
            dir.path(),
            close_calls.clone(),
        )
        .await,
    );

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","reason":"open browser page through remote exit","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let calls = close_calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["op"], "close_stream");
    assert!(calls[0]["stream_id"]
        .as_str()
        .is_some_and(|stream_id| stream_id.starts_with("remote-carrier:mock:test:open-")));
    assert_eq!(calls[0]["principal_id"], authority.principal_id);
}

#[tokio::test]
async fn test_browser_open_failure_retries_pending_remote_exit_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let app = gateway_router(
        rejecting_browser_engine_remote_carrier_exit_test_state_with_close_failures(
            dir.path(),
            close_calls.clone(),
            1,
        )
        .await,
    );

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .method("POST")
                    .uri("/api/apps/browser/open")
                    .header("x-elastos-home-token", token.clone())
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"url":"https://glidefinance.io/","reason":"open browser page through remote exit","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    let calls = close_calls.lock().await;
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[1]["stream_id"], calls[0]["stream_id"]);
    assert_ne!(calls[2]["stream_id"], calls[0]["stream_id"]);
    assert_eq!(calls[0]["principal_id"], authority.principal_id);
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
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"glidefinance.io","reason":"open browser page","viewport":{"width":900,"height":520},"display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
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
    #[cfg(unix)]
    {
        assert!(runtime_stream_path.starts_with("/tmp/elastos-browser-streams"));
        assert!(
            runtime_stream_path.to_string_lossy().len() < 100,
            "runtime stream socket path must fit conservative Unix sun_path budget: {}",
            runtime_stream_path.display()
        );
    }
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
        "webrtc_remote_display"
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
            test_browser_request("localhost:61180", "null")
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
async fn test_browser_async_open_returns_job_and_polls_same_principal_result() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let other_authority = passkey_authority_with_name_role(
        dir.path(),
        Some("Browser Guest"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let other_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &other_authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://slow-open.invalid/","reason":"async Browser open regression","viewport":{"width":900,"height":520},"display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi","async_open":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let accepted: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(accepted["schema"], "elastos.browser.open-accepted/v1");
    let open_id = accepted["open_id"].as_str().unwrap();
    assert!(open_id.starts_with("browser-open:"));
    assert!(open_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_')));
    assert_eq!(
        accepted["status_url"],
        format!("/api/apps/browser/open/{open_id}")
    );

    let blocked = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!("/api/apps/browser/open/{open_id}"))
                .header("x-elastos-home-token", other_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::NOT_FOUND);

    let mut completed = None;
    for _ in 0..10 {
        let status_response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .uri(format!("/api/apps/browser/open/{open_id}"))
                    .header("x-elastos-home-token", token.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status_response.status(), StatusCode::OK);
        let status_body = axum::body::to_bytes(status_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
        assert_eq!(status["schema"], "elastos.browser.open-status/v1");
        if status["status"] == "completed" {
            completed = Some(status);
            break;
        }
        assert_eq!(status["status"], "pending");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let completed = completed.expect("async Browser open should complete");
    let result = &completed["result"];
    assert_eq!(result["schema"], "elastos.browser.open-result/v1");
    assert_eq!(result["target"], "tls://slow-open.invalid:443");
    assert_eq!(
        result["engine_page"]["schema"],
        "elastos.browser.engine.page/v1"
    );
    assert!(result["stream_session"].get("adapter_ipc").is_none());
    assert!(result["stream_session"].get("relay_ipc").is_none());
}

#[tokio::test]
async fn test_browser_async_duplicate_open_coalesces_by_verified_launch_owner() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);
    let body = r#"{"url":"https://slow-open.invalid/","reason":"restored Browser lifecycle","viewport":{"width":900,"height":520},"display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi","async_open":true}"#;

    let first = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_accepted: serde_json::Value = serde_json::from_slice(&first_body).unwrap();

    let duplicate = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::ACCEPTED);
    let duplicate_body = axum::body::to_bytes(duplicate.into_body(), usize::MAX)
        .await
        .unwrap();
    let duplicate_accepted: serde_json::Value = serde_json::from_slice(&duplicate_body).unwrap();
    assert_eq!(duplicate_accepted["open_id"], first_accepted["open_id"]);

    let conflict = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://conflicting-open.invalid/","reason":"different restored Browser lifecycle","viewport":{"width":900,"height":520},"display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi","async_open":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let open_id = first_accepted["open_id"].as_str().unwrap();
    let mut completed = None;
    for _ in 0..20 {
        let status_response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .uri(format!("/api/apps/browser/open/{open_id}"))
                    .header("x-elastos-home-token", token.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status_body = axum::body::to_bytes(status_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
        if status["status"] == "completed" {
            completed = Some(status);
            break;
        }
        assert_eq!(status["status"], "pending");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let completed = completed.expect("coalesced Browser open should complete");
    let page_id = completed["result"]["engine_page"]["page_id"]
        .as_str()
        .unwrap();
    let cleanup_id = browser_cleanup_id(&completed["result"]).to_string();

    let replay = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::ACCEPTED);
    let replay_body = axum::body::to_bytes(replay.into_body(), usize::MAX)
        .await
        .unwrap();
    let replay_accepted: serde_json::Value = serde_json::from_slice(&replay_body).unwrap();
    assert_eq!(replay_accepted["open_id"], first_accepted["open_id"]);
    let replay_id = replay_accepted["open_id"].as_str().unwrap();
    let replay_status = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!("/api/apps/browser/open/{replay_id}"))
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let replay_status_body = axum::body::to_bytes(replay_status.into_body(), usize::MAX)
        .await
        .unwrap();
    let replay_status: serde_json::Value = serde_json::from_slice(&replay_status_body).unwrap();
    assert_eq!(replay_status["status"], "completed");
    assert_eq!(
        replay_status["result"]["engine_page"]["page_id"],
        completed["result"]["engine_page"]["page_id"]
    );

    let completed_conflict = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://post-completion-conflict.invalid/","reason":"different restored Browser lifecycle","viewport":{"width":900,"height":520},"display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi","async_open":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(completed_conflict.status(), StatusCode::CONFLICT);

    let summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/browser/summary")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(summary["sessions"]["active_sessions"], 1);
    assert_eq!(summary["sessions"]["launching_sessions"], 0);

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let open_events = auth_state
        .audit
        .iter()
        .filter(|event| {
            event.capsule_id.as_deref() == Some(BROWSER_CAPSULE_ID)
                && event.event_type.starts_with("browser.open.")
        })
        .collect::<Vec<_>>();
    assert_eq!(open_events.len(), 2);

    let close = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(&cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_browser_open_job_and_page_routes_require_exact_verified_launch_owner() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let owner_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let other_launch_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);
    let open = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", owner_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://exact-owner.invalid/","reason":"exact launch owner test","viewport":{"width":900,"height":520},"display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi","async_open":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(open.status(), StatusCode::ACCEPTED);
    let open_body = axum::body::to_bytes(open.into_body(), usize::MAX)
        .await
        .unwrap();
    let accepted: serde_json::Value = serde_json::from_slice(&open_body).unwrap();
    let open_id = accepted["open_id"].as_str().unwrap();

    let foreign_poll = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!("/api/apps/browser/open/{open_id}"))
                .header("x-elastos-home-token", other_launch_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_poll.status(), StatusCode::NOT_FOUND);

    let mut completed = None;
    for _ in 0..20 {
        let status = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .uri(format!("/api/apps/browser/open/{open_id}"))
                    .header("x-elastos-home-token", owner_token.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(status.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        if payload["status"] == "completed" {
            completed = Some(payload);
            break;
        }
        assert_eq!(payload["status"], "pending");
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let completed = completed.expect("owner Browser open should complete");
    let page_id = completed["result"]["engine_page"]["page_id"]
        .as_str()
        .unwrap();
    let cleanup_id = browser_cleanup_id(&completed["result"]).to_string();
    let encoded_page_id = page_id.replace(':', "%3A");

    let foreign_status = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/status"))
                .header("x-elastos-home-token", other_launch_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_status.status(), StatusCode::NOT_FOUND);

    let foreign_diagnostics = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!(
                    "/api/apps/browser/pages/{encoded_page_id}/diagnostics"
                ))
                .header("x-elastos-home-token", other_launch_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_diagnostics.status(), StatusCode::NOT_FOUND);

    let foreign_heartbeat = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!(
                    "/api/apps/browser/pages/{encoded_page_id}/heartbeat"
                ))
                .header("x-elastos-home-token", other_launch_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_heartbeat.status(), StatusCode::NOT_FOUND);

    let foreign_input = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/input"))
                .header("x-elastos-home-token", other_launch_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"event":{"type":"browser_command","command":"reload"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_input.status(), StatusCode::NOT_FOUND);

    let foreign_webrtc = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/webrtc"))
                .header("x-elastos-home-token", other_launch_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"type":"offer","sdp":"v=0\r\ns=ElastOS Browser Test\r\n"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_webrtc.status(), StatusCode::NOT_FOUND);

    let foreign_close = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/close"))
                .header("x-elastos-home-token", other_launch_token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(&cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_close.status(), StatusCode::NOT_FOUND);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);

    let owner_status = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/status"))
                .header("x-elastos-home-token", owner_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(owner_status.status(), StatusCode::OK);

    let owner_close = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/close"))
                .header("x-elastos-home-token", owner_token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(&cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(owner_close.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_browser_open_launches_selected_engine_adapter() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://ela.city/","adapter_id":"mock-jetson-engine","viewport":{"width":900,"height":520},"display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
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
    assert_eq!(payload["engine_page"]["adapter"], "mock-jetson-engine");
    assert_eq!(payload["engine_page"]["direct_network"], false);
    assert_eq!(payload["engine_page"]["wallet_injection"], false);
}

#[tokio::test]
async fn test_browser_session_capacity_tracks_open_heartbeat_and_close() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let browser_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &token);
    let state = browser_engine_attached_test_state(dir.path()).await;
    let wallet_provider = Arc::new(RecordingWalletProvider::default());
    state
        .provider_registry
        .as_ref()
        .unwrap()
        .register_sub_provider("wallet", wallet_provider.clone())
        .await
        .unwrap();
    let app = gateway_router(state);

    let before_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://example.com/","reason":"session capacity regression","viewport":{"width":900,"height":520},"display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
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
    let cleanup_id = browser_cleanup_id(&opened);
    wallet_provider
        .assert_v2_account_reads(&browser_wallet_authority, 2)
        .await;

    let after_open_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
            test_browser_request("localhost:61180", "null")
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
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close_response.status(), StatusCode::OK);

    let after_close_response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
async fn test_browser_profile_reset_refuses_route_open_live_page() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let open_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://example.com/","reason":"profile reset route live-page regression","viewport":{"width":900,"height":520},"display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
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
    let cleanup_id = browser_cleanup_id(&opened);

    let reset_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/profile/reset")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reset_response.status(), StatusCode::CONFLICT);
    let reset_body = axum::body::to_bytes(reset_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let reset_message = String::from_utf8(reset_body.to_vec()).unwrap();
    assert!(reset_message.contains("requires all Browser pages"));

    let close_response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close_response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_browser_session_capacity_tracks_multiple_pages_for_principal() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let first_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let second_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let app = gateway_router(
        browser_engine_remote_carrier_exit_test_state_with_close_calls(
            dir.path(),
            close_calls.clone(),
        )
        .await,
    );

    let first_open_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", first_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://ela.city/","reason":"first Browser page","viewport":{"width":960,"height":540},"display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
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
    let first_stream_id = first_opened["stream_session"]["stream_id"]
        .as_str()
        .unwrap()
        .to_string();
    let first_cleanup_id = browser_cleanup_id(&first_opened).to_string();

    let second_open_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", second_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://ela.city/","reason":"second Browser page","viewport":{"width":960,"height":540},"display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
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
    let second_stream_id = second_opened["stream_session"]["stream_id"]
        .as_str()
        .unwrap()
        .to_string();
    let second_cleanup_id = browser_cleanup_id(&second_opened).to_string();

    assert_ne!(
        first_page_id, second_page_id,
        "additional launches must create a new tracked page"
    );
    assert_ne!(
        first_stream_id, second_stream_id,
        "additional launches must receive a new Runtime stream"
    );
    {
        let calls = close_calls.lock().await;
        assert_eq!(calls.len(), 0);
    }

    let after_open_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/browser/summary")
                .header("x-elastos-home-token", first_token.clone())
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
    assert_eq!(after_open["sessions"]["max_active_sessions"], 4);
    assert_eq!(after_open["sessions"]["max_sessions_per_principal"], 4);
    assert_eq!(after_open["sessions"]["capacity_available"], true);
    let lifecycle = &after_open["sessions"]["lifecycle"];
    assert_eq!(lifecycle["schema"], "elastos.browser.lifecycle-status/v1");
    assert_eq!(lifecycle["owner"], "runtime_gateway");
    let lifecycle_sessions = lifecycle["sessions"].as_array().unwrap();
    assert_eq!(lifecycle_sessions.len(), 2);
    let lifecycle_text = lifecycle.to_string();
    assert!(
        !lifecycle_text.contains(first_page_id.as_str()),
        "Browser lifecycle must not expose raw page ids"
    );
    assert!(
        !lifecycle_text.contains("person:local"),
        "Browser lifecycle must not expose raw principal ids"
    );
    for session in lifecycle_sessions {
        assert_eq!(session["phase"], "ACTIVE_SESSION");
        assert!(session["page_id"].as_str().unwrap().starts_with("sha256:"));
        assert!(session["session_id"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert!(session["owner_launch_id"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert!(session["principal_id"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert!(session["profile_key_hash"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert_eq!(session["exit_id"], "local-runtime");
        assert_eq!(session["warm_vm"], false);
        assert!(session["age_ms"].as_u64().is_some());
        assert!(session["last_navigation_at"].as_u64().is_some());
        assert!(session["pending_launch_age_ms"].is_null());
        assert!(session["failure_reason"].is_null());
    }

    let first_heartbeat_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{first_page_id}/heartbeat"))
                .header("x-elastos-home-token", first_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_heartbeat_response.status(), StatusCode::OK);
    let first_heartbeat_body =
        axum::body::to_bytes(first_heartbeat_response.into_body(), usize::MAX)
            .await
            .unwrap();
    let first_heartbeat: serde_json::Value = serde_json::from_slice(&first_heartbeat_body).unwrap();
    assert_eq!(
        first_heartbeat["schema"],
        "elastos.browser.page-heartbeat/v1"
    );
    assert_eq!(
        first_heartbeat["page_id"].as_str(),
        Some(first_page_id.as_str())
    );

    let second_heartbeat_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!(
                    "/api/apps/browser/pages/{second_page_id}/heartbeat"
                ))
                .header("x-elastos-home-token", second_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_heartbeat_response.status(), StatusCode::OK);
    let second_heartbeat_body =
        axum::body::to_bytes(second_heartbeat_response.into_body(), usize::MAX)
            .await
            .unwrap();
    let second_heartbeat: serde_json::Value =
        serde_json::from_slice(&second_heartbeat_body).unwrap();
    assert_eq!(
        second_heartbeat["schema"],
        "elastos.browser.page-heartbeat/v1"
    );
    assert_eq!(
        second_heartbeat["page_id"].as_str(),
        Some(second_page_id.as_str())
    );

    let first_close_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{first_page_id}/close"))
                .header("x-elastos-home-token", first_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(&first_cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_close_response.status(), StatusCode::OK);
    {
        let calls = close_calls.lock().await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["op"], "close_stream");
        assert_eq!(calls[0]["stream_id"], first_stream_id);
        assert_eq!(calls[0]["principal_id"], authority.principal_id);
    }

    let second_close_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{second_page_id}/close"))
                .header("x-elastos-home-token", second_token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(&second_cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_close_response.status(), StatusCode::OK);
    {
        let calls = close_calls.lock().await;
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1]["op"], "close_stream");
        assert_eq!(calls[1]["stream_id"], second_stream_id);
        assert_eq!(calls[1]["principal_id"], authority.principal_id);
    }

    let after_all_close_response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/browser/summary")
                .header("x-elastos-home-token", first_token)
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
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","reason":"open browser page","display_mode":"native_surface","guarantee_level":"policy_webview"}"#,
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
    assert!(message.contains("native_surface"));
}

#[tokio::test]
async fn test_browser_open_reports_engine_capacity_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://capacity-unavailable.invalid/","reason":"capacity unavailable regression","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["schema"], "elastos.browser.open-error/v1");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["code"], "browser_capacity_unavailable");
    assert_eq!(
        payload["outcome"]["schema"],
        "elastos.browser.open-outcome/v1"
    );
    assert_eq!(payload["outcome"]["state"], "terminal_post_effect_cleanup");
    assert_eq!(payload["outcome"]["effects"]["page_acquired"], false);
    assert_eq!(payload["outcome"]["effects"]["vm_acquired"], false);
    assert_eq!(payload["outcome"]["effects"]["stream_acquired"], true);
    assert_eq!(browser_page_session_count(dir.path()).await, 0);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(browser_stream_cleanup_obligation_count(dir.path()).await, 0);
}

#[tokio::test]
async fn test_browser_async_open_reports_structured_post_dispatch_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let accepted = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://resources-in-use.invalid/","reason":"structured post-dispatch cleanup regression","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi","async_open":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let accepted = axum::body::to_bytes(accepted.into_body(), usize::MAX)
        .await
        .unwrap();
    let accepted: serde_json::Value = serde_json::from_slice(&accepted).unwrap();
    let open_id = accepted["open_id"].as_str().unwrap();

    let mut failed = None;
    for _ in 0..20 {
        let response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .uri(format!("/api/apps/browser/open/{open_id}"))
                    .header("x-elastos-home-token", token.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: serde_json::Value = serde_json::from_slice(&body).unwrap();
        if status["status"] == "failed" {
            failed = Some(status);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let failed = failed.expect("async Browser open should return a terminal failure");
    assert_eq!(
        failed["error"]["outcome"]["state"],
        "terminal_post_effect_cleanup"
    );
    assert_eq!(failed["error"]["code"], "resources_in_use");
    assert_eq!(
        failed["error"]["outcome"]["effects"]["page_acquired"],
        false
    );
    assert_eq!(failed["error"]["outcome"]["effects"]["vm_acquired"], false);
    assert_eq!(
        failed["error"]["outcome"]["effects"]["stream_acquired"],
        true
    );
    assert_eq!(browser_page_session_count(dir.path()).await, 0);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(browser_stream_cleanup_obligation_count(dir.path()).await, 0);
}

#[tokio::test]
async fn test_browser_resources_in_use_requires_exact_did_not_act_reconciliation() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let (state, close_calls, reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::DidNotActResourcesInUse,
    )
    .await;
    let app = gateway_router(state);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://resources-in-use.invalid/","reason":"exact DidNotAct reconciliation","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["code"], "resources_in_use");
    assert_eq!(payload["outcome"]["state"], "terminal_post_effect_cleanup");
    assert_eq!(payload["outcome"]["effects"]["page_acquired"], false);
    assert_eq!(payload["outcome"]["effects"]["vm_acquired"], false);
    assert_eq!(payload["outcome"]["effects"]["stream_acquired"], true);
    assert_eq!(
        reconciliation_calls.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(close_calls.lock().await.len(), 0);
    assert_eq!(browser_page_session_count(dir.path()).await, 0);
    assert_eq!(
        browser_launch_reconciliation_obligation_count(dir.path()).await,
        0
    );
}

fn write_browser_vz_transport_test_config(data_dir: &std::path::Path) {
    let config_dir = data_dir.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let path = config_dir.join("browser-vz-vsock-transport.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "schema": "elastos.browser.vz-transport-config/v1",
            "enabled": true,
            "turn_listen_host": "127.0.0.1",
            "turn_advertised_host": "127.0.0.1",
            "turn_relay_host": "127.0.0.1",
            "turn_port_start": 43300,
            "turn_port_end": 43331,
            "turn_relay_port_start": 43400,
            "turn_relay_port_end": 43527,
            "turn_relay_block_size": 8,
            "guest_turn_host": "127.0.0.1",
            "guest_turn_port": 3478,
            "bootstrap_vsock_port": 19090,
            "egress_vsock_port": 19091,
            "media_vsock_port": 19093,
            "ttl_secs": 300
        }))
        .unwrap(),
    )
    .unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
}

#[tokio::test]
async fn test_exact_vz_did_not_act_releases_all_runtime_obligations_without_reconciliation() {
    let dir = tempfile::tempdir().unwrap();
    write_browser_vz_transport_test_config(dir.path());
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let (state, close_calls, reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::ExactVzDidNotAct,
    )
    .await;
    let app = gateway_router(state);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://exact-vz-did-not-act.invalid/","reason":"consume exact VZ DidNotAct","display_mode":"webrtc_remote_display","guarantee_level":"mechanism_microvm"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let response_status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        response_status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{payload}"
    );
    assert_eq!(payload["outcome"]["state"], "terminal_post_effect_cleanup");
    assert_eq!(payload["outcome"]["effects"]["page_acquired"], false);
    assert_eq!(payload["outcome"]["effects"]["vm_acquired"], false);
    assert_eq!(payload["outcome"]["effects"]["stream_acquired"], true);
    assert_eq!(
        reconciliation_calls.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(close_calls.lock().await.is_empty());
    assert_eq!(browser_page_session_count(dir.path()).await, 0);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(browser_stream_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(
        browser_launch_reconciliation_obligation_count(dir.path()).await,
        0
    );
    assert!(!dir.path().join("browser-lifecycle").exists());
    assert!(!dir.path().join("browser-streams").exists());
}

#[tokio::test]
async fn test_mismatched_vz_did_not_act_remains_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    write_browser_vz_transport_test_config(dir.path());
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let (state, close_calls, reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::MismatchedVzDidNotAct,
    )
    .await;
    let app = gateway_router(state);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://mismatched-vz-did-not-act.invalid/","reason":"reject substituted VZ DidNotAct","display_mode":"webrtc_remote_display","guarantee_level":"mechanism_microvm"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let response_status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        response_status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{payload}"
    );
    assert_eq!(payload["outcome"]["state"], "cleanup_pending");
    assert_eq!(
        payload["outcome"]["ownership"],
        "launch_reconciliation_pending"
    );
    assert_eq!(
        reconciliation_calls.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert!(close_calls.lock().await.is_empty());
    assert_eq!(
        browser_launch_reconciliation_obligation_count(dir.path()).await,
        1
    );
}

#[tokio::test]
async fn test_exact_terminal_vz_settlement_releases_restart_reconciliation_obligations() {
    let dir = tempfile::tempdir().unwrap();
    write_browser_vz_transport_test_config(dir.path());
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let (state, close_calls, reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::TerminalVzSettlement,
    )
    .await;
    let app = gateway_router(state);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://terminal-vz-settlement.invalid/","reason":"consume exact terminal VZ settlement","display_mode":"webrtc_remote_display","guarantee_level":"mechanism_microvm"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["outcome"]["state"], "terminal_post_effect_cleanup");
    assert_eq!(payload["outcome"]["effects"]["page_acquired"], false);
    assert_eq!(payload["outcome"]["effects"]["vm_acquired"], true);
    assert_eq!(
        reconciliation_calls.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert!(close_calls.lock().await.is_empty());
    assert_eq!(browser_page_session_count(dir.path()).await, 0);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(browser_stream_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(
        browser_launch_reconciliation_obligation_count(dir.path()).await,
        0
    );
    assert!(!dir.path().join("browser-lifecycle").exists());
    assert!(!dir.path().join("browser-streams").exists());
}

#[tokio::test]
async fn test_mismatched_terminal_vz_settlement_retains_restart_reconciliation_ownership() {
    let dir = tempfile::tempdir().unwrap();
    write_browser_vz_transport_test_config(dir.path());
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let (state, close_calls, reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::MismatchedTerminalVzSettlement,
    )
    .await;
    let app = gateway_router(state);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://mismatched-terminal-vz.invalid/","reason":"retain mismatched terminal VZ settlement","display_mode":"webrtc_remote_display","guarantee_level":"mechanism_microvm"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["outcome"]["state"], "cleanup_pending");
    assert_eq!(
        payload["outcome"]["ownership"],
        "launch_reconciliation_pending"
    );
    assert_eq!(
        reconciliation_calls.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert!(close_calls.lock().await.is_empty());
    assert_eq!(
        browser_launch_reconciliation_obligation_count(dir.path()).await,
        1
    );
}

#[tokio::test]
async fn test_browser_launch_response_loss_reconciles_and_closes_exact_effect() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let (state, close_calls, reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::ResponseLoss,
    )
    .await;
    let app = gateway_router(state);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://response-loss.invalid/","reason":"response loss reconciliation","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["outcome"]["state"], "terminal_post_effect_cleanup");
    assert_eq!(payload["outcome"]["effects"]["page_acquired"], true);
    assert_eq!(payload["outcome"]["effects"]["vm_acquired"], true);
    assert_eq!(close_calls.lock().await.len(), 1);
    assert_eq!(
        reconciliation_calls.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(browser_page_session_count(dir.path()).await, 0);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(
        browser_launch_reconciliation_obligation_count(dir.path()).await,
        0
    );
}

#[tokio::test]
async fn test_browser_malformed_success_reconciles_instead_of_releasing_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let (state, close_calls, reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::MalformedSuccess,
    )
    .await;
    let app = gateway_router(state);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://malformed-success.invalid/","reason":"malformed success reconciliation","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["outcome"]["state"], "terminal_post_effect_cleanup");
    assert_eq!(close_calls.lock().await.len(), 1);
    assert_eq!(
        reconciliation_calls.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        browser_launch_reconciliation_obligation_count(dir.path()).await,
        0
    );
}

#[tokio::test]
async fn test_browser_provider_crash_blocks_until_terminal_proof_then_allows_new_generation() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let retry_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let terminal_retry_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let (state, close_calls, reconciliation_calls) = browser_engine_reconciliation_test_state(
        dir.path(),
        MockDispatchedBrowserLaunchFailure::PendingThenTerminal,
    )
    .await;
    let app = gateway_router(state);

    let first = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://provider-crash.invalid/","reason":"provider crash reconciliation","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::SERVICE_UNAVAILABLE);
    let first = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    let first: serde_json::Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(first["outcome"]["state"], "cleanup_pending");
    assert_eq!(
        first["outcome"]["effects"]["page_acquired"],
        serde_json::Value::Null
    );
    assert_eq!(
        first["outcome"]["effects"]["vm_acquired"],
        serde_json::Value::Null
    );
    assert_eq!(
        first["outcome"]["ownership"],
        "launch_reconciliation_pending"
    );
    assert_eq!(close_calls.lock().await.len(), 0);
    assert_eq!(
        browser_launch_reconciliation_obligation_count(dir.path()).await,
        1
    );

    let replacement = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", retry_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://replacement.invalid/","reason":"must remain blocked","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replacement.status(), StatusCode::SERVICE_UNAVAILABLE);
    let replacement = axum::body::to_bytes(replacement.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8(replacement.to_vec())
        .unwrap()
        .contains("cleanup is pending"));
    assert!(reconciliation_calls.load(std::sync::atomic::Ordering::SeqCst) >= 2);
    assert_eq!(
        browser_launch_reconciliation_obligation_count(dir.path()).await,
        1
    );

    let terminal_replacement = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", terminal_retry_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://replacement-after-terminal.invalid/","reason":"terminal proof permits a new generation","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(terminal_replacement.status(), StatusCode::OK);
    let terminal_replacement = axum::body::to_bytes(terminal_replacement.into_body(), usize::MAX)
        .await
        .unwrap();
    let terminal_replacement: serde_json::Value =
        serde_json::from_slice(&terminal_replacement).unwrap();
    assert_eq!(
        terminal_replacement["engine_page"]["schema"],
        "elastos.browser.engine.page/v1"
    );
    assert_eq!(close_calls.lock().await.len(), 0);
    assert!(reconciliation_calls.load(std::sync::atomic::Ordering::SeqCst) >= 3);
    assert_eq!(
        browser_launch_reconciliation_obligation_count(dir.path()).await,
        0
    );
    assert_eq!(browser_page_session_count(dir.path()).await, 1);
}

#[tokio::test]
async fn test_browser_personal_sign_queues_wallet_inbox_approval() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let browser_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &browser_token);
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
    let (state, wallet_provider) =
        wallet_test_state_with_recording_provider(dir.path(), provider).await;
    let app = gateway_router(state);

    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/request-signature")
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
    wallet_provider
        .assert_v2_account_reads(&browser_wallet_authority, 1)
        .await;
    wallet_provider
        .assert_v2_approval_operations(
            &browser_wallet_authority,
            &[WalletOperationKind::RequestApproval],
        )
        .await;

    let inbox = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/request-signature")
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
            test_browser_request("localhost:61180", "null")
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
    let browser_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &browser_token);
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);
    let address = "0x1111111111111111111111111111111111111111";
    let account_id = format!("wallet:eip155:20:{address}");
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::new(vec![json!({
            "account_id": account_id,
            "principal_id": authority.principal_id.clone(),
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
    let (state, wallet_provider) =
        wallet_chain_test_state_with_recording_wallet_provider(dir.path(), provider).await;
    let app = gateway_router(state);

    let block_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/read")
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
            test_browser_request("localhost:61180", "null")
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
            test_browser_request("localhost:61180", "null")
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
            test_browser_request("localhost:61180", "null")
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

    let tx_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
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
        (
            "eth_getTransactionByHash",
            json!([tx_hash]),
            json!({
                "hash": tx_hash,
                "from": address,
                "to": "0x2222222222222222222222222222222222222222",
                "value": "0x1",
                "blockNumber": "0x2a"
            }),
        ),
        (
            "eth_getTransactionReceipt",
            json!([tx_hash]),
            json!({
                "transactionHash": tx_hash,
                "status": "0x1",
                "blockNumber": "0x2a",
                "logs": []
            }),
        ),
    ];
    for (method, params, expected) in extra_reads {
        let response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
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
    wallet_provider
        .assert_v2_account_reads(&browser_wallet_authority, 11)
        .await;

    let inbox = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
        22,
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
    let browser_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &browser_token);
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
    let (state, wallet_provider) =
        wallet_chain_test_state_with_recording_wallet_provider(dir.path(), provider).await;
    let app = gateway_router(state);

    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
    assert_eq!(
        payload["approval_request"]["payload"]["method"],
        "eth_sendTransaction"
    );
    assert_eq!(
        payload["approval_request"]["payload"]["page_url"],
        "https://glidefinance.io/"
    );
    assert_eq!(
        payload["approval_request"]["payload"]["origin"],
        "https://glidefinance.io"
    );
    wallet_provider
        .assert_v2_account_reads(&browser_wallet_authority, 1)
        .await;
    wallet_provider
        .assert_v2_approval_operations(
            &browser_wallet_authority,
            &[WalletOperationKind::RequestApproval],
        )
        .await;

    let inbox = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
            test_browser_request("localhost:61180", "null")
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
    assert_eq!(
        payload["approval_request"]["payload"]["method"],
        "eth_sendTransaction"
    );
    assert_eq!(
        payload["approval_request"]["payload"]["page_url"],
        "https://glidefinance.io/"
    );
    assert_eq!(
        payload["approval_request"]["payload"]["origin"],
        "https://glidefinance.io"
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
            test_browser_request("localhost:61180", "null")
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

fn completed_browser_transaction_approval(
    authority: &RuntimeWalletAuthority,
    request_id: &str,
    signed_transaction: &str,
    transaction_hash: &str,
    reason: &str,
) -> serde_json::Value {
    let context = authority.verified_context();
    let payload = json!({
        "schema": "elastos.chain.unsigned_transaction_intent/v1",
        "network": { "id": "esc-mainnet" },
        "from": "0x1111111111111111111111111111111111111111",
        "to": "0x2222222222222222222222222222222222222222",
        "value": "0x1",
        "data": "0x",
        "wallet_intent": "transaction_intent"
    });
    let operation = WalletProviderOperationV2::RequestApproval {
        account_id: "wallet:eip155:20:0x1111111111111111111111111111111111111111".to_string(),
        chain_namespace: "eip155:20".to_string(),
        intent: "transaction_intent".to_string(),
        resource: "elastos://chain/esc-mainnet/broadcast_transaction".to_string(),
        reason: reason.to_string(),
        payload: payload.clone(),
        expires_at: 20,
    };
    let wallet_request_sha256 = WalletProviderRequestV2::new(context, request_id, 1, 2, operation)
        .unwrap()
        .request_sha256;
    json!({
        "schema": "elastos.wallet.approval_request/v1",
        "request_id": request_id,
        "wallet_request_sha256": wallet_request_sha256,
        "authority_binding": format!("0x{}", "c".repeat(64)),
        "kind": "signature",
        "status": "completed",
        "intent": "transaction_intent",
        "capsule_id": BROWSER_CAPSULE_ID,
        "requested_by_actor": BROWSER_CAPSULE_ID,
        "resource": "elastos://chain/esc-mainnet/broadcast_transaction",
        "reason": reason,
        "account_id": "wallet:eip155:20:0x1111111111111111111111111111111111111111",
        "chain_namespace": "eip155:20",
        "address": "0x1111111111111111111111111111111111111111",
        "proof_binding_id": "proof:wallet:managed:eip155:20:0x1111111111111111111111111111111111111111",
        "proof_type": "managed_evm",
        "payload_hash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "principal_id": context.principal_id(),
        "session_id": context.session_id(),
        "launch_id": context.launch_id(),
        "created_at": 10,
        "expires_at": 20,
        "completed_at": 12,
        "payload": payload,
        "signed_result": {
            "schema": "elastos.wallet.signed-transaction-result/v1",
            "request_id": request_id,
            "method": "eth_sendTransaction",
            "signed_transaction": signed_transaction,
            "transaction_hash": transaction_hash,
            "signer": "0x1111111111111111111111111111111111111111",
            "payload_hash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "chain_namespace": "eip155:20"
        }
    })
}

#[tokio::test]
async fn test_browser_completed_transaction_approval_broadcasts_through_chain_provider() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let browser_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &browser_token);
    let request_id = "wallet-request:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![completed_browser_transaction_approval(
            &browser_wallet_authority,
            request_id,
            "0x02f8",
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "Browser page requests eth_sendTransaction on esc-mainnet",
        )]),
        defaults: TokioMutex::default(),
    };
    let app =
        gateway_router(wallet_chain_test_state_with_wallet_provider(dir.path(), provider).await);

    let status_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/browser/wallet/approvals/wallet-request%3Abbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
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
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"request_id":"wallet-request:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}"#,
                ))
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
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/browser/wallet/approvals/wallet-request%3Abbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
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
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"request_id":"wallet-request:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}"#,
                ))
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
async fn test_browser_transaction_broadcast_record_failure_does_not_rebroadcast_on_retry() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let browser_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &browser_token);
    let request_id = "wallet-request:cccccccccccccccccccccccccccccccc";
    let signed_transaction = "0xdeadbeef0017";
    reset_mock_chain_broadcast_count(signed_transaction);
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![completed_browser_transaction_approval(
            &browser_wallet_authority,
            request_id,
            signed_transaction,
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "Browser page requests eth_sendTransaction on esc-mainnet; record-fails",
        )]),
        defaults: TokioMutex::default(),
    };
    let app =
        gateway_router(wallet_chain_test_state_with_wallet_provider(dir.path(), provider).await);

    let first_response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"request_id":"wallet-request:cccccccccccccccccccccccccccccccc"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_response.status(), StatusCode::OK);
    let first_body = axum::body::to_bytes(first_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
    assert_eq!(first_json["completion_status"], "pending");
    assert!(first_json["completion_error"]
        .as_str()
        .is_some_and(|message| message.contains("projection")));
    assert_eq!(mock_chain_broadcast_count(signed_transaction), 1);

    let retry_response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"request_id":"wallet-request:cccccccccccccccccccccccccccccccc"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retry_response.status(), StatusCode::OK);
    let retry_body = axum::body::to_bytes(retry_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let retry_json: serde_json::Value = serde_json::from_slice(&retry_body).unwrap();
    assert_eq!(retry_json["completion_status"], "complete");
    assert_eq!(mock_chain_broadcast_count(signed_transaction), 1);
}

#[tokio::test]
async fn test_browser_uncertain_broadcast_reconciles_by_wallet_hash_without_rebroadcast() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let browser_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &browser_token);
    let request_id = "wallet-request:11111111111111111111111111111111";
    let signed_transaction = "0xfeedface0001";
    let transaction_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    reset_mock_chain_broadcast_count(signed_transaction);
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![completed_browser_transaction_approval(
            &browser_wallet_authority,
            request_id,
            signed_transaction,
            transaction_hash,
            "Browser uncertain broadcast recovery",
        )]),
        defaults: TokioMutex::default(),
    };
    let state = wallet_chain_test_state_with_wallet_provider(dir.path(), provider).await;
    let app = gateway_router(state.clone());

    let first = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"request_id":"{request_id}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(mock_chain_broadcast_count(signed_transaction), 1);
    let effect_store = transaction_effect_store_for_test(
        &state,
        browser_wallet_authority.verified_context().principal_id(),
    );
    assert!(effect_store["effects"]
        .as_array()
        .unwrap()
        .iter()
        .all(|effect| effect.get("signed_transaction").is_none()));

    let retry = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"request_id":"{request_id}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retry.status(), StatusCode::OK);
    let body = axum::body::to_bytes(retry.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["transaction_hash"], transaction_hash);
    assert_eq!(payload["completion_status"], "complete");
    assert_eq!(mock_chain_broadcast_count(signed_transaction), 1);
}

#[tokio::test]
async fn test_browser_chain_provider_unavailable_recovers_after_restart_without_broadcast() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let browser_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &browser_token);
    let request_id = "wallet-request:55555555555555555555555555555555";
    let signed_transaction = "0xdecafbad0003";
    let transaction_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    reset_mock_chain_broadcast_count(signed_transaction);
    let approval = completed_browser_transaction_approval(
        &browser_wallet_authority,
        request_id,
        signed_transaction,
        transaction_hash,
        "Browser provider-unavailable recovery",
    );
    let unavailable_provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![approval.clone()]),
        defaults: TokioMutex::default(),
    };
    let unavailable_app =
        gateway_router(wallet_test_state_with_provider(dir.path(), unavailable_provider).await);

    let first = unavailable_app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"request_id":"{request_id}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::SERVICE_UNAVAILABLE);
    let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&first_body).contains("chain provider unavailable"));
    assert_eq!(mock_chain_broadcast_count(signed_transaction), 0);

    let restarted_provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![approval]),
        defaults: TokioMutex::default(),
    };
    let restarted_app = gateway_router(
        wallet_chain_test_state_with_wallet_provider(dir.path(), restarted_provider).await,
    );
    let recovered = restarted_app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"request_id":"{request_id}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(recovered.status(), StatusCode::OK);
    let recovered_body = axum::body::to_bytes(recovered.into_body(), usize::MAX)
        .await
        .unwrap();
    let recovered_json: serde_json::Value = serde_json::from_slice(&recovered_body).unwrap();
    assert_eq!(recovered_json["transaction_hash"], transaction_hash);
    assert_eq!(recovered_json["completion_status"], "complete");
    assert_eq!(mock_chain_broadcast_count(signed_transaction), 0);
}

#[tokio::test]
async fn test_browser_stale_wallet_operation_binding_is_rejected_before_broadcast() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let browser_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &browser_token);
    let request_id = "wallet-request:66666666666666666666666666666666";
    let signed_transaction = "0xdecafbad0004";
    reset_mock_chain_broadcast_count(signed_transaction);
    let mut approval = completed_browser_transaction_approval(
        &browser_wallet_authority,
        request_id,
        signed_transaction,
        "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "Browser stale operation binding",
    );
    approval["wallet_request_sha256"] = json!(format!("request:sha256:{}", "d".repeat(64)));
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![approval]),
        defaults: TokioMutex::default(),
    };
    let app =
        gateway_router(wallet_chain_test_state_with_wallet_provider(dir.path(), provider).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"request_id":"{request_id}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("stale or substituted"));
    assert_eq!(mock_chain_broadcast_count(signed_transaction), 0);
}

#[tokio::test]
async fn test_browser_indeterminate_broadcast_never_rebroadcasts_without_chain_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let browser_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &browser_token);
    let request_id = "wallet-request:22222222222222222222222222222222";
    let signed_transaction = "0xfeedface0002";
    let transaction_hash = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    reset_mock_chain_broadcast_count(signed_transaction);
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![completed_browser_transaction_approval(
            &browser_wallet_authority,
            request_id,
            signed_transaction,
            transaction_hash,
            "Browser indeterminate broadcast",
        )]),
        defaults: TokioMutex::default(),
    };
    let app =
        gateway_router(wallet_chain_test_state_with_wallet_provider(dir.path(), provider).await);

    for attempt in 0..2 {
        let response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .method("POST")
                    .uri("/api/apps/browser/wallet/broadcast-transaction")
                    .header("x-elastos-home-token", browser_token.clone())
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(r#"{{"request_id":"{request_id}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        if attempt == 1 {
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert!(String::from_utf8_lossy(&body).contains("will not rebroadcast"));
        }
        assert_eq!(mock_chain_broadcast_count(signed_transaction), 1);
    }
}

#[tokio::test]
async fn test_browser_chain_receipt_hash_mismatch_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let browser_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &browser_token);
    let request_id = "wallet-request:33333333333333333333333333333333";
    let signed_transaction = "0xdecafbad0001";
    let wallet_hash = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    reset_mock_chain_broadcast_count(signed_transaction);
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![completed_browser_transaction_approval(
            &browser_wallet_authority,
            request_id,
            signed_transaction,
            wallet_hash,
            "Browser receipt mismatch",
        )]),
        defaults: TokioMutex::default(),
    };
    let app =
        gateway_router(wallet_chain_test_state_with_wallet_provider(dir.path(), provider).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"request_id":"{request_id}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("Wallet-computed hash"));
    assert_eq!(mock_chain_broadcast_count(signed_transaction), 1);
}

#[tokio::test]
async fn test_browser_audit_failure_is_pending_and_retry_does_not_rebroadcast() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let browser_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &browser_token);
    let request_id = "wallet-request:44444444444444444444444444444444";
    let signed_transaction = "0xdecafbad0002";
    let transaction_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    reset_mock_chain_broadcast_count(signed_transaction);
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![completed_browser_transaction_approval(
            &browser_wallet_authority,
            request_id,
            signed_transaction,
            transaction_hash,
            "Browser audit-fails once",
        )]),
        defaults: TokioMutex::default(),
    };
    let app =
        gateway_router(wallet_chain_test_state_with_wallet_provider(dir.path(), provider).await);

    for (attempt, expected_status) in [(0, "pending"), (1, "complete")] {
        let response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .method("POST")
                    .uri("/api/apps/browser/wallet/broadcast-transaction")
                    .header("x-elastos-home-token", browser_token.clone())
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(r#"{{"request_id":"{request_id}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["completion_status"], expected_status);
        if attempt == 0 {
            assert!(payload["completion_error"]
                .as_str()
                .is_some_and(|message| message.contains("audit")));
        }
        assert_eq!(mock_chain_broadcast_count(signed_transaction), 1);
    }
}

async fn external_browser_transaction_app(
    data_dir: &std::path::Path,
    transaction_hash: &str,
    reason: &str,
) -> (axum::Router, String, String) {
    let authority = passkey_authority(data_dir);
    let browser_token = app_token_for_authority(data_dir, BROWSER_CAPSULE_ID, &authority);
    let browser_wallet_authority =
        runtime_wallet_authority_for_app_token(data_dir, BROWSER_CAPSULE_ID, &browser_token);
    let context = browser_wallet_authority.verified_context();
    let request_id = "wallet-request:77777777777777777777777777777777";
    let payload = json!({
        "schema": "elastos.chain.unsigned_transaction_intent/v1",
        "network": { "id": "esc-mainnet" },
        "from": "0x3333333333333333333333333333333333333333",
        "to": "0x2222222222222222222222222222222222222222",
        "value": "0x1",
        "data": "0x",
        "wallet_intent": "transaction_intent"
    });
    let wallet_request_sha256 = WalletProviderRequestV2::new(
        context,
        request_id,
        1,
        2,
        WalletProviderOperationV2::RequestApproval {
            account_id: "wallet:eip155:20:0x3333333333333333333333333333333333333333".to_string(),
            chain_namespace: "eip155:20".to_string(),
            intent: "transaction_intent".to_string(),
            resource: "elastos://chain/esc-mainnet/broadcast_transaction".to_string(),
            reason: reason.to_string(),
            payload: payload.clone(),
            expires_at: 20,
        },
    )
    .unwrap()
    .request_sha256;
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![json!({
            "schema": "elastos.wallet.approval_request/v1",
            "request_id": request_id,
            "wallet_request_sha256": wallet_request_sha256,
            "authority_binding": format!("0x{}", "e".repeat(64)),
            "kind": "signature",
            "status": "completed",
            "intent": "transaction_intent",
            "capsule_id": BROWSER_CAPSULE_ID,
            "requested_by_actor": BROWSER_CAPSULE_ID,
            "resource": "elastos://chain/esc-mainnet/broadcast_transaction",
            "reason": reason,
            "account_id": "wallet:eip155:20:0x3333333333333333333333333333333333333333",
            "chain_namespace": "eip155:20",
            "address": "0x3333333333333333333333333333333333333333",
            "proof_binding_id": "proof:wallet:siwe:eip155:20:0x3333333333333333333333333333333333333333",
            "proof_type": "siwe",
            "connector_id": "wallet-metamask",
            "payload_hash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "payload": payload,
            "principal_id": context.principal_id(),
            "session_id": context.session_id(),
            "launch_id": context.launch_id(),
            "created_at": 10,
            "expires_at": 20,
            "completed_at": 12,
            "signed_result": {
                "schema": "elastos.wallet.external-transaction-result/v1",
                "request_id": request_id,
                "method": "eth_sendTransaction",
                "transaction_hash": transaction_hash,
                "signer": "0x3333333333333333333333333333333333333333",
                "chain_namespace": "eip155:20",
                "payload_hash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            }
        })]),
        defaults: TokioMutex::default(),
    };
    (
        gateway_router(wallet_chain_test_state_with_wallet_provider(data_dir, provider).await),
        browser_token,
        request_id.to_string(),
    )
}

#[tokio::test]
async fn test_browser_requested_audit_failure_prevents_managed_chain_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let browser_wallet_authority =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &browser_token);
    let request_id = "wallet-request:88888888888888888888888888888888";
    let signed_transaction = "0xdecafbad0011";
    let transaction_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    reset_mock_chain_broadcast_count(signed_transaction);
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![completed_browser_transaction_approval(
            &browser_wallet_authority,
            request_id,
            signed_transaction,
            transaction_hash,
            "Browser requested-audit-fails",
        )]),
        defaults: TokioMutex::default(),
    };
    let app =
        gateway_router(wallet_chain_test_state_with_wallet_provider(dir.path(), provider).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"request_id":"{request_id}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(String::from_utf8_lossy(&body).contains("requested audit"));
    assert_eq!(mock_chain_broadcast_count(signed_transaction), 0);
}

#[tokio::test]
async fn test_browser_external_transaction_completes_only_after_chain_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let tx_hash = "0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    let (app, browser_token, request_id) = external_browser_transaction_app(
        dir.path(),
        tx_hash,
        "Browser page requests eth_sendTransaction on esc-mainnet",
    )
    .await;

    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/browser/wallet/approvals/wallet-request%3A77777777777777777777777777777777")
                .header("x-elastos-home-token", browser_token.clone())
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

    let broadcast = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"request_id":"{request_id}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(broadcast.status(), StatusCode::OK);
    let body = axum::body::to_bytes(broadcast.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["transaction_hash"], tx_hash);
    assert_eq!(payload["recorded"], false);
    assert_eq!(payload["already_recorded"], false);
    assert_eq!(payload["completion_status"], "complete");
    assert_eq!(
        payload["receipt"]["transaction"]["from"],
        "0x3333333333333333333333333333333333333333"
    );

    let replay = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/broadcast-transaction")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"request_id":"{request_id}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let body = axum::body::to_bytes(replay.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["already_recorded"], true);
    assert_eq!(payload["completion_status"], "complete");
}

#[tokio::test]
async fn test_browser_external_transaction_remains_indeterminate_without_chain_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let transaction_hash = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let (app, browser_token, request_id) = external_browser_transaction_app(
        dir.path(),
        transaction_hash,
        "Browser external transaction without Chain evidence",
    )
    .await;

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .method("POST")
                    .uri("/api/apps/browser/wallet/broadcast-transaction")
                    .header("x-elastos-home-token", browser_token.clone())
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(r#"{{"request_id":"{request_id}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(String::from_utf8_lossy(&body).contains("never rebroadcasts"));
    }
}

#[tokio::test]
async fn test_browser_external_transaction_rejects_chain_hash_and_from_substitution() {
    for (transaction_hash, expected_error) in [
        (
            "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "payload hash mismatch",
        ),
        (
            "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "originating account mismatch",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (app, browser_token, request_id) = external_browser_transaction_app(
            dir.path(),
            transaction_hash,
            "Browser external transaction substitution",
        )
        .await;
        let response = app
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .method("POST")
                    .uri("/api/apps/browser/wallet/broadcast-transaction")
                    .header("x-elastos-home-token", browser_token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(r#"{{"request_id":"{request_id}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(
            String::from_utf8_lossy(&body).contains(expected_error),
            "unexpected substitution response: {}",
            String::from_utf8_lossy(&body)
        );
    }
}

#[tokio::test]
async fn test_browser_page_runtime_routes_are_runtime_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let open = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"glidefinance.io","reason":"runtime scoped page route test","viewport":{"width":900,"height":520},"display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(open.status(), StatusCode::OK);
    let body = axum::body::to_bytes(open.into_body(), usize::MAX)
        .await
        .unwrap();
    let opened: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let page_id = opened["engine_page"]["page_id"].as_str().unwrap();
    let cleanup_id = browser_cleanup_id(&opened);
    let encoded_page_id = page_id.replace(':', "%3A");

    let status = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/status"))
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
    assert_eq!(payload["page_id"], page_id);
    assert_eq!(payload["direct_network"], false);
    assert_eq!(
        payload["display_session"]["schema"],
        "elastos.browser.display-session/v1"
    );
    assert_eq!(
        payload["display_session"]["media_transport"],
        "runtime_relay"
    );
    assert_eq!(
        payload["display_session"]["ice_servers"][0]["credential_present"],
        true
    );
    assert!(payload["display_session"]["ice_servers"][0]
        .get("credential")
        .is_none());
    assert_eq!(payload["webrtc_connection_state"], "connected");

    let diagnostics = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!(
                    "/api/apps/browser/pages/{encoded_page_id}/diagnostics"
                ))
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(diagnostics.status(), StatusCode::OK);
    let body = axum::body::to_bytes(diagnostics.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["schema"], "elastos.browser.page-diagnostics/v1");
    assert_eq!(payload["page_id"], page_id);
    assert_eq!(payload["direct_network"], false);
    assert_eq!(payload["image_count"], 3);
    assert_eq!(payload["broken_image_count"], 1);
    assert_eq!(payload["viewport_width"], 1280);
    assert_eq!(payload["viewport_height"], 720);
    assert_eq!(payload["clickable_count"], 1);
    assert_eq!(payload["clickable_elements"][0]["text"], "Directory");
    assert_eq!(
        payload["clickable_elements"][0]["href"],
        "https://glidefinance.io/directory"
    );

    let input = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/input"))
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
    assert_eq!(payload["page_id"], page_id);
    assert_eq!(payload["accepted"], true);

    let close = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/close"))
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(cleanup_id))
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
    assert_eq!(payload["page_id"], page_id);
    assert_eq!(payload["closed"], true);

    let input_after_close = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/input"))
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"event":{"type":"click","x":12,"y":34}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(input_after_close.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_browser_page_routes_require_page_owner() {
    let dir = tempfile::tempdir().unwrap();
    let owner = passkey_authority_with_name(dir.path(), Some("owner"));
    let other = passkey_authority_with_name_role(
        dir.path(),
        Some("other"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let owner_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &owner);
    let other_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &other);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);
    let opened =
        open_mock_browser_page_result(app.clone(), &owner_token, "principal-owned page route test")
            .await;
    let page_id = opened["engine_page"]["page_id"].as_str().unwrap();
    let cleanup_id = browser_cleanup_id(&opened);
    let encoded_page_id = page_id.replace(':', "%3A");

    let status = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/status"))
                .header("x-elastos-home-token", other_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::NOT_FOUND);

    let diagnostics = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!(
                    "/api/apps/browser/pages/{encoded_page_id}/diagnostics"
                ))
                .header("x-elastos-home-token", other_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(diagnostics.status(), StatusCode::NOT_FOUND);

    let heartbeat = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!(
                    "/api/apps/browser/pages/{encoded_page_id}/heartbeat"
                ))
                .header("x-elastos-home-token", other_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(heartbeat.status(), StatusCode::NOT_FOUND);

    let input = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/input"))
                .header("x-elastos-home-token", other_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"event":{"type":"click","x":12,"y":34}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(input.status(), StatusCode::NOT_FOUND);

    let webrtc = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/webrtc"))
                .header("x-elastos-home-token", other_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"type":"offer","sdp":"v=0\r\ns=ElastOS Browser Test\r\n"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(webrtc.status(), StatusCode::NOT_FOUND);

    let close = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/close"))
                .header("x-elastos-home-token", other_token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close.status(), StatusCode::NOT_FOUND);

    let owner_status = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/status"))
                .header("x-elastos-home-token", owner_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(owner_status.status(), StatusCode::OK);

    let owner_close = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/close"))
                .header("x-elastos-home-token", owner_token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(owner_close.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_browser_close_failure_retires_runtime_page_session() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);
    let opened = open_mock_browser_page_result(app.clone(), &token, "simulate close failure").await;
    let page_id = opened["engine_page"]["page_id"].as_str().unwrap();
    let cleanup_id = browser_cleanup_id(&opened);
    let encoded_page_id = page_id.replace(':', "%3A");

    let close = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/close"))
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let status_after_failed_close = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/status"))
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status_after_failed_close.status(), StatusCode::NOT_FOUND);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 1);
    assert!(browser_principal_has_live_sessions(dir.path(), &authority.principal_id).await);
}

#[tokio::test]
async fn test_browser_no_first_frame_cleanup_retries_exact_effect_without_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let owner_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let replacement_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let engine_close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let exit_close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let ownership = Arc::new(MockBrowserOwnershipCounts::default());
    let app = gateway_router(
        browser_engine_retrying_close_test_state(
            dir.path(),
            engine_close_calls.clone(),
            MockBrowserEngineCloseFailure::Transport,
            2,
            Some((exit_close_calls.clone(), 0)),
            Some(ownership.clone()),
        )
        .await,
    );
    let opened =
        open_mock_browser_page_result(app.clone(), &owner_token, "no first frame cleanup").await;
    let page_id = opened["engine_page"]["page_id"].as_str().unwrap();
    let cleanup_id = browser_cleanup_id(&opened);
    assert_eq!(
        opened["runtime_cleanup"]["schema"],
        "elastos.browser.cleanup-handle/v1"
    );
    assert_eq!(ownership.first_video_frame_count(), 0);
    assert_eq!(ownership.snapshot(), (1, 1, 1, 1, 1));

    let close_body = json!({
        "schema": "elastos.browser.close-request/v2",
        "cleanup_id": cleanup_id,
    });
    let first_close = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", owner_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(close_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_close.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(browser_page_session_count(dir.path()).await, 0);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 1);
    assert_eq!(browser_stream_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(engine_close_calls.lock().await.len(), 1);
    assert_eq!(exit_close_calls.lock().await.len(), 1);
    assert_eq!(ownership.snapshot(), (1, 1, 1, 0, 1));

    let replacement = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", replacement_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://replacement.invalid/","reason":"must remain blocked","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replacement.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(browser_page_session_count(dir.path()).await, 0);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 1);
    assert_eq!(engine_close_calls.lock().await.len(), 2);
    assert_eq!(ownership.snapshot(), (1, 1, 1, 0, 1));

    let terminal_close = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", owner_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(close_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(terminal_close.status(), StatusCode::OK);
    let terminal_body = axum::body::to_bytes(terminal_close.into_body(), usize::MAX)
        .await
        .unwrap();
    let terminal: serde_json::Value = serde_json::from_slice(&terminal_body).unwrap();
    assert_eq!(terminal["cleanup_id"], cleanup_id);
    assert_eq!(engine_close_calls.lock().await.len(), 3);
    assert_eq!(browser_page_session_count(dir.path()).await, 0);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(browser_stream_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(ownership.first_video_frame_count(), 0);
    assert!(!browser_principal_has_live_sessions(dir.path(), &authority.principal_id).await);
    assert_eq!(ownership.snapshot(), (1, 0, 0, 0, 0));

    let status = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!("/api/apps/browser/pages/{page_id}/status"))
                .header("x-elastos-home-token", owner_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_browser_pending_cleanup_rejects_foreign_authority_and_effect_substitution() {
    let dir = tempfile::tempdir().unwrap();
    let owner = passkey_authority_with_name(dir.path(), Some("cleanup-owner"));
    let foreign = passkey_authority_with_name_role(
        dir.path(),
        Some("cleanup-foreign"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let owner_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &owner);
    let foreign_launch_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &owner);
    let foreign_principal_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &foreign);
    let close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let state = browser_engine_retrying_close_test_state(
        dir.path(),
        close_calls.clone(),
        MockBrowserEngineCloseFailure::Transport,
        1,
        None,
        None,
    )
    .await;
    let registry = state.provider_registry.as_ref().unwrap().clone();
    let app = gateway_router(state);
    let opened =
        open_mock_browser_page_result(app.clone(), &owner_token, "exact cleanup substitution")
            .await;
    let page_id = opened["engine_page"]["page_id"].as_str().unwrap();
    let cleanup_id = browser_cleanup_id(&opened);
    let close_body = json!({
        "schema": "elastos.browser.close-request/v2",
        "cleanup_id": cleanup_id,
    });

    let first_close = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", owner_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(close_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_close.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(close_calls.lock().await.len(), 1);

    for token in [&foreign_principal_token, &foreign_launch_token] {
        let response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .method("POST")
                    .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                    .header("x-elastos-home-token", token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(close_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    let foreign_page = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/pages/page%3Aforeign/close")
                .header("x-elastos-home-token", owner_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(close_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_page.status(), StatusCode::NOT_FOUND);

    let substituted = json!({
        "schema": "elastos.browser.close-request/v2",
        "cleanup_id": "browser-cleanup:substituted",
    });
    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", owner_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(substituted.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(close_calls.lock().await.len(), 1);

    let foreign_provider_close_calls = Arc::new(TokioMutex::new(Vec::new()));
    registry
        .register_sub_provider(
            "browser-engine",
            Arc::new(MockForeignIdentityBrowserEngineProvider {
                close_calls: foreign_provider_close_calls.clone(),
            }),
        )
        .await
        .unwrap();
    let substituted_provider = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", owner_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(close_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        substituted_provider.status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(close_calls.lock().await.len(), 1);
    assert_eq!(foreign_provider_close_calls.lock().await.len(), 0);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 1);

    let restored_close_calls = Arc::new(TokioMutex::new(Vec::new()));
    registry
        .register_sub_provider(
            "browser-engine",
            Arc::new(MockRetryingBrowserEngineProvider::new(
                restored_close_calls.clone(),
                MockBrowserEngineCloseFailure::Transport,
                0,
            )),
        )
        .await
        .unwrap();
    let terminal = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", owner_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(close_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(terminal.status(), StatusCode::OK);
    assert_eq!(restored_close_calls.lock().await.len(), 1);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);
}

#[tokio::test]
async fn test_browser_close_transport_and_exit_failures_retry_independent_cleanup_obligations() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let owner_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let retry_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let engine_close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let exit_close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let app = gateway_router(
        browser_engine_retrying_close_test_state(
            dir.path(),
            engine_close_calls.clone(),
            MockBrowserEngineCloseFailure::Transport,
            1,
            Some((exit_close_calls.clone(), 1)),
            None,
        )
        .await,
    );
    let open_body = r#"{"url":"https://cleanup-owner.invalid/","reason":"independent cleanup test","remote_exit_id":"mock-remote-carrier-exit","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#;
    let open = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", owner_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(open_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(open.status(), StatusCode::OK);
    let open = axum::body::to_bytes(open.into_body(), usize::MAX)
        .await
        .unwrap();
    let open: serde_json::Value = serde_json::from_slice(&open).unwrap();
    let page_id = open["engine_page"]["page_id"].as_str().unwrap();
    let cleanup_id = browser_cleanup_id(&open);

    let close = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", owner_token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 1);
    assert_eq!(browser_stream_cleanup_obligation_count(dir.path()).await, 1);
    assert_eq!(engine_close_calls.lock().await.len(), 1);
    assert_eq!(exit_close_calls.lock().await.len(), 1);

    let retry_open = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", retry_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://cleanup-retry.invalid/","reason":"retry independent cleanup","remote_exit_id":"mock-remote-carrier-exit","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retry_open.status(), StatusCode::OK);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(browser_stream_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(engine_close_calls.lock().await.len(), 2);
    assert_eq!(exit_close_calls.lock().await.len(), 2);
    let retry_open = axum::body::to_bytes(retry_open.into_body(), usize::MAX)
        .await
        .unwrap();
    let retry_open: serde_json::Value = serde_json::from_slice(&retry_open).unwrap();
    let retry_page_id = retry_open["engine_page"]["page_id"].as_str().unwrap();
    let retry_cleanup_id = browser_cleanup_id(&retry_open);

    let terminal_close = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{retry_page_id}/close"))
                .header("x-elastos-home-token", retry_token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(retry_cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(terminal_close.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_browser_close_adapter_error_retries_until_terminal_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let owner_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let retry_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let app = gateway_router(
        browser_engine_retrying_close_test_state(
            dir.path(),
            close_calls.clone(),
            MockBrowserEngineCloseFailure::Adapter,
            1,
            None,
            None,
        )
        .await,
    );
    let opened =
        open_mock_browser_page_result(app.clone(), &owner_token, "adapter cleanup retry").await;
    let page_id = opened["engine_page"]["page_id"].as_str().unwrap();
    let cleanup_id = browser_cleanup_id(&opened);
    let close = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", owner_token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 1);
    assert_eq!(close_calls.lock().await.len(), 1);

    let retry_opened =
        open_mock_browser_page_result(app.clone(), &retry_token, "adapter cleanup terminal retry")
            .await;
    let retry_page = retry_opened["engine_page"]["page_id"].as_str().unwrap();
    let retry_cleanup_id = browser_cleanup_id(&retry_opened);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(close_calls.lock().await.len(), 2);

    let close_retry_page = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{retry_page}/close"))
                .header("x-elastos-home-token", retry_token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(retry_cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close_retry_page.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_browser_close_typed_already_absent_is_terminal_without_retry_obligation() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let close_calls = Arc::new(TokioMutex::new(Vec::new()));
    let app = gateway_router(
        browser_engine_retrying_close_test_state(
            dir.path(),
            close_calls.clone(),
            MockBrowserEngineCloseFailure::AlreadyClosed,
            1,
            None,
            None,
        )
        .await,
    );
    let opened =
        open_mock_browser_page_result(app.clone(), &token, "typed already absent cleanup").await;
    let page_id = opened["engine_page"]["page_id"].as_str().unwrap();
    let cleanup_id = browser_cleanup_id(&opened);
    let close = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close.status(), StatusCode::OK);
    let close_body = axum::body::to_bytes(close.into_body(), usize::MAX)
        .await
        .unwrap();
    let close: serde_json::Value = serde_json::from_slice(&close_body).unwrap();
    assert_eq!(close["closed"], true);
    assert_eq!(close["cleanup_id"], cleanup_id);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(close_calls.lock().await.len(), 1);
}

#[tokio::test]
async fn test_browser_refresh_recovers_and_closes_exact_instance_bound_page() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let owner_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let refreshed_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let owner_launch_id =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &owner_token)
            .verified_context()
            .launch_id()
            .to_string();
    let refreshed_launch_id =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &refreshed_token)
            .verified_context()
            .launch_id()
            .to_string();
    assert_ne!(owner_launch_id, refreshed_launch_id);
    let browser_instance = "browser:0123456789abcdef0123456789abcdef";
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let opened = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", owner_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "url": "glidefinance.io",
                        "reason": "Browser refresh recovery",
                        "browser_instance": browser_instance,
                        "viewport": {"width": 900, "height": 520},
                        "display_mode": "webrtc_remote_display",
                        "guarantee_level": "operator_rbi"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let opened_status = opened.status();
    let opened_body = axum::body::to_bytes(opened.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        opened_status,
        StatusCode::OK,
        "Browser open failed: {}",
        String::from_utf8_lossy(&opened_body)
    );
    let opened: serde_json::Value = serde_json::from_slice(&opened_body).unwrap();
    let page_id = opened["engine_page"]["page_id"].as_str().unwrap();
    let cleanup_id = browser_cleanup_id(&opened);

    let foreign = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/browser/summary?browser_instance=browser%3Afedcba9876543210fedcba9876543210")
                .header("x-elastos-home-token", refreshed_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign.status(), StatusCode::OK);
    let foreign: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(foreign.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(foreign["sessions"]["recoverable_page"].is_null());

    let summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!(
                    "/api/apps/browser/summary?browser_instance={browser_instance}"
                ))
                .header("x-elastos-home-token", refreshed_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let summary: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(summary.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(summary["sessions"]["recoverable_page"]["state"], "active");
    assert_eq!(summary["sessions"]["recoverable_page"]["page_id"], page_id);
    assert_eq!(
        summary["sessions"]["recoverable_page"]["cleanup"]["id"],
        cleanup_id
    );

    let close = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{page_id}/close"))
                .header("x-elastos-home-token", refreshed_token)
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(cleanup_id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close.status(), StatusCode::OK);
    assert_eq!(browser_page_session_count(dir.path()).await, 0);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 0);
    assert_eq!(browser_stream_cleanup_obligation_count(dir.path()).await, 0);
}

#[tokio::test]
async fn test_browser_close_provider_unavailable_keeps_bounded_engine_cleanup_obligation() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let runtime_authority =
        runtime_wallet_authority_for_app_token(dir.path(), BROWSER_CAPSULE_ID, &token);
    let owner_launch_id = runtime_authority.verified_context().launch_id().to_string();
    let reservation = reserve_browser_launch(
        dir.path(),
        &authority.principal_id,
        BrowserLaunchLifecycle {
            owner_launch_id,
            browser_instance: None,
            url: "https://provider-unavailable.invalid/".to_string(),
            exit_id: "local-runtime".to_string(),
            engine_route_provider: "mock-browser-engine".to_string(),
            selected_engine_adapter: Some("mock-adapter".to_string()),
            profile_key_hash: browser_lifecycle_hash("provider-unavailable-profile"),
            vm_key_hash: browser_lifecycle_hash("provider-unavailable-vm"),
        },
    )
    .await
    .unwrap();
    let cleanup = complete_browser_launch(
        dir.path(),
        &reservation,
        BrowserLaunchEffect {
            page_id: "page:provider-unavailable".to_string(),
            engine_provider: "browser-engine-adapter".to_string(),
            engine_protocol_version: BROWSER_ENGINE_PROTOCOL_VERSION.to_string(),
            engine_adapter: "mock-adapter".to_string(),
            engine: "mock-engine".to_string(),
            provider_cleanup: serde_json::json!({
                "schema": BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA,
                "page_id": "page:provider-unavailable",
                "generation": reservation.generation(),
                "stream_id": "stream:provider-unavailable",
                "adapter": "mock-adapter",
                "engine": "mock-engine",
            }),
            browser_page: serde_json::json!({"page_id": "page:provider-unavailable"}),
            stream_cleanup: None,
        },
    )
    .await
    .unwrap();
    let app = gateway_router(test_state(dir.path()));

    let close = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/pages/page%3Aprovider-unavailable/close")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(browser_close_body(&cleanup.id))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(browser_engine_cleanup_obligation_count(dir.path()).await, 1);

    let status = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/browser/pages/page%3Aprovider-unavailable/status")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_browser_webrtc_signal_requires_open_page() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains("browser session is not active"));
}

#[tokio::test]
async fn test_browser_webrtc_signal_is_runtime_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);
    let page_id =
        open_mock_browser_page(app.clone(), &token, "Browser WebRTC offer route test").await;
    let encoded_page_id = page_id.replace(':', "%3A");

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/webrtc"))
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
    assert_eq!(payload["page_id"], page_id);
    assert_eq!(payload["type"], "answer");
    assert!(payload["sdp"].as_str().unwrap().starts_with("v=0"));
}

#[tokio::test]
async fn test_browser_webrtc_offer_rejects_embedded_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);
    let page_id = open_mock_browser_page(
        app.clone(),
        &token,
        "Browser WebRTC invalid offer route test",
    )
    .await;
    let encoded_page_id = page_id.replace(':', "%3A");

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/webrtc"))
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
    let page_id =
        open_mock_browser_page(app.clone(), &token, "Browser WebRTC answer route test").await;
    let encoded_page_id = page_id.replace(':', "%3A");

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/webrtc"))
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
    assert_eq!(payload["page_id"], page_id);
    assert_eq!(payload["type"], "answer");
}

#[tokio::test]
async fn test_browser_webrtc_candidate_signal_is_runtime_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_attached_test_state(dir.path()).await);
    let page_id =
        open_mock_browser_page(app.clone(), &token, "Browser WebRTC candidate route test").await;
    let encoded_page_id = page_id.replace(':', "%3A");

    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/webrtc"))
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
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/webrtc"))
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
    let page_id =
        open_mock_browser_page(app.clone(), &token, "Browser WebRTC provider error test").await;
    let encoded_page_id = page_id.replace(':', "%3A");

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/apps/browser/pages/{encoded_page_id}/webrtc"))
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
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","reason":"open browser page","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
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
    let relay_task = tokio::spawn(async move {
        for (stream_id, target, host, request, response) in [
            (
                "stream:proxy:first",
                "tls://example.com:443",
                "example.com",
                *b"ping",
                *b"pong",
            ),
            (
                "stream:proxy:second",
                "tls://ipfs.ela.city:443",
                "ipfs.ela.city",
                *b"next",
                *b"done",
            ),
        ] {
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
            assert_eq!(header["stream_id"], stream_id);
            assert_eq!(header["target"], target);
            assert_eq!(header["host"], host);
            let mut received = [0_u8; 4];
            relay.read_exact(&mut received).await.unwrap();
            assert_eq!(received, request);
            tokio::io::AsyncWriteExt::write_all(&mut relay, &response)
                .await
                .unwrap();
        }
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
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/open")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"url":"https://glidefinance.io/","reason":"open browser page","display_mode":"webrtc_remote_display","guarantee_level":"operator_rbi"}"#,
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
    for (stream_id, target, host, request, expected) in [
        (
            "stream:proxy:first",
            "tls://example.com:443",
            "example.com",
            *b"ping",
            *b"pong",
        ),
        (
            "stream:proxy:second",
            "tls://ipfs.ela.city:443",
            "ipfs.ela.city",
            *b"next",
            *b"done",
        ),
    ] {
        let mut stream = UnixStream::connect(&socket_path).await.unwrap();
        let mut open = serde_json::to_vec(&serde_json::json!({
            "schema": "elastos.exit.relay-open/v1",
            "stream_id": stream_id,
            "target": target,
            "scheme": "tls",
            "host": host,
            "reason": "test browser proxy request",
        }))
        .unwrap();
        open.push(b'\n');
        tokio::io::AsyncWriteExt::write_all(&mut stream, &open)
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut stream, &request)
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
        assert_eq!(response, expected);
    }
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
    });

    let forbidden = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
            test_browser_request("localhost:61180", "null")
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
            test_browser_request("localhost:61180", "null")
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
            test_browser_request("localhost:61180", "null")
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
async fn test_browser_net_stream_provider_route_is_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(net_exit_test_state(dir.path()).await);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
    assert_eq!(response.status(), StatusCode::GONE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let message = String::from_utf8(body.to_vec()).unwrap();
    assert!(message.contains("/api/provider/net/stream is disabled"));
    assert!(message.contains("/api/apps/browser/open"));
}

#[tokio::test]
async fn test_raw_browser_engine_and_exit_provider_proxy_routes_are_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let app = gateway_router(browser_engine_test_state(dir.path()).await);

    for route in [
        "/api/provider/browser-engine/launch",
        "/api/provider/browser-engine/page_status",
        "/api/provider/exit/open_stream",
    ] {
        let response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .method("POST")
                    .uri(route)
                    .header("x-elastos-home-token", browser_token.clone())
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "route: {route}");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let message = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            message.contains("Gateway provider not found"),
            "route {route} returned {message}"
        );
    }
}
