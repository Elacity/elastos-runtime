use super::*;

const HOME_CLI_CAPSULE_ID_FOR_TEST: &str = "home-cli";

struct EnvRestore {
    key: &'static str,
    previous: Option<String>,
}

impl EnvRestore {
    fn set(key: &'static str, value: String) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

#[tokio::test]
async fn test_home_static_route_serves_browser_surface() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    std::fs::write(
        dir.path()
            .join("capsules")
            .join(SYSTEM_CAPSULE_ID)
            .join("browser")
            .join("esp-projections.mjs"),
        "export const ok = true;",
    )
    .unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/apps/home/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .all(|value| !value.starts_with(&format!("{HOME_SESSION_COOKIE}="))),
        "Home index should not auto-mint a local Home session cookie"
    );
    assert_eq!(
        resp.headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/html; charset=utf-8")
    );
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("Home · ElastOS"));
    assert!(text.contains("./home-shell-host.js"));

    let unsigned_summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unsigned_summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(unsigned_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["authority"]["signed_in"], false);

    let valid_cookie = format!("{}={}", HOME_SESSION_COOKIE, home_app_token(dir.path()));
    let existing_session = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/apps/home/")
                .header(COOKIE, valid_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(existing_session.status(), StatusCode::OK);
    assert!(
        existing_session
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .all(|value| !value.starts_with(&format!("{HOME_SESSION_COOKIE}="))),
        "valid Home session cookie should not be replaced"
    );

    let asset = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/apps/home/home-shell-host.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(asset.status(), StatusCode::OK);
    assert_eq!(
        asset
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/javascript")
    );

    let module_asset = app
        .oneshot(
            Request::builder()
                .uri("/apps/system/esp-projections.mjs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(module_asset.status(), StatusCode::OK);
    assert_eq!(
        module_asset
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/javascript")
    );
}

#[tokio::test]
#[cfg(unix)]
async fn test_home_cli_terminal_stream_requires_cli_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    let _program = EnvRestore::set("ELASTOS_HOME_CLI_TERMINAL_PROGRAM", "/bin/sh".to_string());
    let _args = EnvRestore::set(
        "ELASTOS_HOME_CLI_TERMINAL_ARGS_JSON",
        serde_json::json!([
            "-c",
            "printf 'ready\\n'; while IFS= read -r line; do printf 'echo:%s\\n' \"$line\"; [ \"$line\" = exit ] && exit 0; done"
        ])
        .to_string(),
    );
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("terminal"));
    let home_token = authority.home_token.clone();
    let cli_token = app_token_for_authority(dir.path(), HOME_CLI_CAPSULE_ID_FOR_TEST, &authority);

    let contract = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home-cli/terminal/contract")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(contract.status(), StatusCode::OK);
    let body = axum::body::to_bytes(contract.into_body(), usize::MAX)
        .await
        .unwrap();
    let contract: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(contract["schema"], "elastos.home-cli.terminal-contract/v1");
    assert_eq!(contract["transport"], "runtime_pty_stream");
    assert!(contract["renderer_contract"]
        .as_str()
        .unwrap()
        .contains("xterm.js"));
    assert!(contract["pty"]
        .as_str()
        .unwrap()
        .contains("Runtime-owned PTY"));

    let missing = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home-cli/terminal/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::FORBIDDEN);

    let wrong_app = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home-cli/terminal/sessions")
                .header("x-elastos-home-token", home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_app.status(), StatusCode::FORBIDDEN);

    let started = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home-cli/terminal/sessions")
                .header("x-elastos-home-token", cli_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "schema": "elastos.home-cli.terminal-start/v1",
                        "cols": 132,
                        "rows": 36
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK);
    let body = axum::body::to_bytes(started.into_body(), usize::MAX)
        .await
        .unwrap();
    let started: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(started["schema"], "elastos.home-cli.terminal-session/v1");
    assert_eq!(started["transport"], "runtime_pty_stream");
    assert_eq!(started["pty"], true);
    assert_eq!(started["authority"]["app"], HOME_CLI_CAPSULE_ID_FOR_TEST);
    assert_eq!(started["dimensions"]["cols"], 132);
    assert_eq!(started["dimensions"]["rows"], 36);
    assert_eq!(started["process"]["mode"], "tui");
    assert_eq!(started["stream"]["schema"], "elastos.runtime.stream/v1");
    assert_eq!(
        started["stream"]["resize_schema"],
        "elastos.home-cli.terminal-resize/v1"
    );
    assert!(started["stream"]["resize_url"]
        .as_str()
        .unwrap()
        .ends_with("/resize"));
    let session_id = started["session_id"].as_str().unwrap();
    let events_url = started["stream"]["events_url"].as_str().unwrap();
    let input_url = started["stream"]["input_url"].as_str().unwrap();
    let resize_url = started["stream"]["resize_url"].as_str().unwrap();
    let close_url = started["stream"]["close_url"].as_str().unwrap();
    assert!(events_url.contains(session_id));
    assert!(!events_url.contains("home_token="));
    assert!(events_url.contains("ticket="));

    let bad_events = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/apps/home-cli/terminal/sessions/{session_id}/events?ticket=wrong"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_events.status(), StatusCode::FORBIDDEN);

    let wrong_input = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(input_url)
                .header("x-elastos-home-token", home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "schema": "elastos.home-cli.terminal-input/v1",
                        "data": "hello\n"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_input.status(), StatusCode::FORBIDDEN);

    let resize = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(resize_url)
                .header("x-elastos-home-token", cli_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "schema": "elastos.home-cli.terminal-resize/v1",
                        "cols": 90,
                        "rows": 24
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resize.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resize.into_body(), usize::MAX)
        .await
        .unwrap();
    let resize: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(resize["schema"], "elastos.home-cli.terminal-resize/v1");
    assert_eq!(resize["dimensions"]["cols"], 90);
    assert_eq!(resize["dimensions"]["rows"], 24);

    let input = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(input_url)
                .header("x-elastos-home-token", cli_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "schema": "elastos.home-cli.terminal-input/v1",
                        "data": "exit\n"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(input.status(), StatusCode::OK);
    let body = axum::body::to_bytes(input.into_body(), usize::MAX)
        .await
        .unwrap();
    let input: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(input["schema"], "elastos.home-cli.terminal-input/v1");
    assert_eq!(input["session_id"], session_id);

    let closed = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(close_url)
                .header("x-elastos-home-token", cli_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(closed.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_home_summary_reports_identity_and_launch_targets() {
    let dir = tempfile::tempdir().unwrap();

    let state = library_test_state(dir.path()).await;
    let app = gateway_router(state);
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
    let library_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let public = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(public.status(), StatusCode::OK);
    let public_body = axum::body::to_bytes(public.into_body(), usize::MAX)
        .await
        .unwrap();
    let public_payload: serde_json::Value = serde_json::from_slice(&public_body).unwrap();
    assert_eq!(public_payload["authority"]["signed_in"], false);
    assert_eq!(public_payload["authority"]["principal_id"], "");
    assert_eq!(public_payload["authority"]["session_id"], "");
    assert_eq!(public_payload["authority"]["wallet_connected"], false);
    assert!(public_payload["identity"]["handle"].is_null());
    assert!(public_payload["identity"]["device_did"].is_null());
    assert_eq!(public_payload["browser_state"]["principal_id"], "");
    assert_eq!(public_payload["browser_state"]["localhost_root"], "");
    assert_eq!(
        public_payload["desktop_objects"]["schema"],
        "elastos.home.desktop-objects/v1"
    );
    assert_eq!(public_payload["desktop_objects"]["uri"], "");
    assert!(public_payload["desktop_objects"]["objects"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(public_payload["browser_state"]["layout"].is_null());
    assert!(public_payload["browser_state"]["session"].is_null());
    assert!(public_payload["browser_state"]["recent_targets"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(public_payload["appearance"]["background_image_url"].is_null());
    assert_eq!(
        public_payload["appearance"]["background_overlay_enabled"],
        false
    );
    assert_eq!(public_payload["runtime"]["running"], false);
    assert_eq!(public_payload["notifications"]["unread_count"], 0);
    assert!(public_payload["targets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|target| target["target"] == "system"));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/object/mkdir")
                .header("x-elastos-home-token", library_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "parent_uri": format!("{}/Desktop", crate::auth::principal_localhost_root(&authority.principal_id)),
                        "name": "Test Folder",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["authority"]["signed_in"], true);
    assert_eq!(payload["identity"]["handle"], "anders");
    assert!(payload["identity"]["device_did"].is_string());
    assert_eq!(payload["home"]["route"], "/apps/home/");
    assert_eq!(payload["home"]["attach_kind"], "iframe");
    assert_eq!(payload["app"]["id"], "home");
    assert_eq!(payload["app"]["route"], "/apps/home/");
    assert!(payload["appearance"]["background_image_url"].is_null());
    assert_eq!(payload["runtime"]["running"], false);
    assert_eq!(payload["site"]["root_uri"], MY_WEBSITE_URI);
    assert_eq!(payload["room"]["pending_count"], 0);
    assert_eq!(payload["notifications"]["unread_count"], 0);
    assert_eq!(
        payload["desktop_objects"]["schema"],
        "elastos.home.desktop-objects/v1"
    );
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    assert_eq!(
        payload["desktop_objects"]["uri"],
        format!("{localhost_root}/Desktop")
    );
    assert!(payload["desktop_objects"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|object| object["name"] == "Test Folder" && object["kind"] == "directory"));
    assert!(payload["desktop_objects"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|object| {
            object["name"] == "Trash"
                && object["kind"] == "directory"
                && object["uri"] == format!("{localhost_root}/.Trash")
                && object["metadata"]["system_kind"] == "trash"
        }));
    let targets = payload["targets"].as_array().unwrap();
    let system = targets
        .iter()
        .find(|target| target["target"] == "system")
        .expect("system target");
    assert_eq!(system["role"], "app");
    assert_eq!(system["title"], "System");
    assert_eq!(
        system["description"],
        "Manage passkeys, appearance, and runtime settings for this Home."
    );
    assert_eq!(system["route"], "/apps/system/");
    assert_eq!(system["attach_kind"], "iframe");
    assert_eq!(system["target_kind"], "app");
    let services = targets
        .iter()
        .find(|target| target["target"] == "services")
        .expect("services target");
    assert_eq!(services["role"], "app");
    assert_eq!(services["title"], "Services");
    assert_eq!(
        services["description"],
        "Manage Browser Exit Node sharing and subscriptions."
    );
    assert_eq!(services["route"], "/apps/services/");
    assert_eq!(services["attach_kind"], "iframe");
    assert_eq!(services["target_kind"], "app");
    assert!(targets
        .iter()
        .any(|target| target["target"] == "chat-room" && target["role"] == "app"));
    let library = targets
        .iter()
        .find(|target| target["target"] == "library")
        .expect("library target");
    assert_eq!(library["role"], "app");
    assert_eq!(library["title"], "Library");
    assert_eq!(
        library["description"],
        "Browse documents and open them in Documents."
    );
    assert_eq!(library["route"], "/apps/library/");
    assert_eq!(library["attach_kind"], "iframe");
    assert_eq!(library["target_kind"], "app");
    let inbox = targets
        .iter()
        .find(|target| target["target"] == "inbox")
        .expect("inbox target");
    assert_eq!(inbox["role"], "app");
    assert_eq!(inbox["title"], "Inbox");
    assert_eq!(
        inbox["description"],
        "Review requests and approvals for this Home."
    );
    assert_eq!(inbox["route"], "/apps/inbox/");
    assert_eq!(inbox["attach_kind"], "iframe");
    assert_eq!(inbox["target_kind"], "app");
    let wallet = targets
        .iter()
        .find(|target| target["target"] == "wallet")
        .expect("wallet target");
    assert_eq!(wallet["role"], "app");
    assert_eq!(wallet["title"], "Wallet");
    assert_eq!(
        wallet["description"],
        "View accounts, balances, approvals, and approval methods."
    );
    assert_eq!(wallet["route"], "/apps/wallet/");
    assert_eq!(wallet["attach_kind"], "iframe");
    assert_eq!(wallet["target_kind"], "app");
    let browser = targets
        .iter()
        .find(|target| target["target"] == "browser")
        .expect("browser target");
    assert_eq!(browser["role"], "app");
    assert_eq!(browser["title"], "Browser");
    assert_eq!(
        browser["description"],
        "Open web sites through the ElastOS Browser boundary."
    );
    assert_eq!(browser["route"], "/apps/browser/");
    assert_eq!(browser["attach_kind"], "iframe");
    assert_eq!(browser["target_kind"], "app");
    assert!(targets
        .iter()
        .all(|target| target["target"] != "wallet-metamask"));
    assert!(targets
        .iter()
        .all(|target| target["target"] != "wallet-unisat"));
    assert!(targets
        .iter()
        .all(|target| target["target"] != "wallet-walletconnect"));
    assert!(targets
        .iter()
        .any(|target| target["target"] == "gba-ucity" && target["target_kind"] == "object"));
    assert_eq!(
        targets
            .iter()
            .filter(|target| target["target"] == "system")
            .count(),
        1
    );
    assert_eq!(
        targets
            .iter()
            .filter(|target| target["target"] == "library")
            .count(),
        1
    );
    assert_eq!(
        targets
            .iter()
            .filter(|target| target["target"] == "chat-room")
            .count(),
        1
    );
    assert_eq!(
        targets
            .iter()
            .filter(|target| target["target"] == "inbox")
            .count(),
        1
    );
    assert_eq!(
        targets
            .iter()
            .filter(|target| target["target"] == "services")
            .count(),
        1
    );
}

#[tokio::test]
async fn test_services_summary_requires_services_token_and_reports_browser_exit() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(dir.path().join("config/exit-provider.json"), "{}").unwrap();

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let ok = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/services/summary")
                .header(
                    "x-elastos-home-token",
                    issue_home_launch_token(dir.path(), SERVICES_CAPSULE_ID).unwrap(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let body = axum::body::to_bytes(ok.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["schema"], "elastos.runtime.services/v1");
    assert_eq!(payload["local_offer_count"], 0);
    assert_eq!(payload["local_offers"].as_array().unwrap().len(), 0);
    assert!(payload["available_local_offers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|offer| {
            offer["offer_id"] == "local:provider:browser-exit"
                && offer["display_name"] == "Browser Exit node"
                && offer["service_kind"] == "remote_exit"
                && offer["enabled"] == false
                && offer["status"] == "available"
        }));

    let enabled = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/services/offers")
                .header(
                    "x-elastos-home-token",
                    issue_home_launch_token(dir.path(), SERVICES_CAPSULE_ID).unwrap(),
                )
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"offer_id":"local:provider:browser-exit","section":"mine","selected":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(enabled.status(), StatusCode::OK);
    let body = axum::body::to_bytes(enabled.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["local_offer_count"], 1);
    assert!(payload["local_offers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|offer| {
            offer["offer_id"] == "local:provider:browser-exit" && offer["enabled"] == true
        }));
}

#[tokio::test]
async fn test_services_summary_projects_configured_remote_exit_without_ticket() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/exit-provider.json"),
        serde_json::to_vec_pretty(&json!({
            "backends": [],
            "remote_carrier_exits": [{
                "id": "mac-browser-exit",
                "grant_id": "operator-grant:mac-browser-exit:test",
                "peer_did": "did:key:z6Mkmac",
                "carrier_service": "elastos://exit/open_stream",
                "connect_ticket": "ticket:must-not-leak",
                "allowed_principals": ["person:local:test"],
                "allowed_hosts": ["*"],
                "allowed_schemes": ["tcp", "tls"],
                "allowed_ports": [80, 443],
                "max_active_streams": 4,
                "max_active_streams_per_principal": 2
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let token = issue_home_launch_token(dir.path(), SERVICES_CAPSULE_ID).unwrap();

    let ok = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let body = axum::body::to_bytes(ok.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains("connect_ticket"));
    assert!(!text.contains("ticket:must-not-leak"));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["remote_offer_count"], 1);
    assert_eq!(payload["available_remote_offer_count"], 0);
    let offer = payload["remote_offers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|offer| offer["source"] == "configured_remote_exit")
        .expect("configured remote Exit should be projected as active");
    assert_eq!(offer["offer_id"], "configured:remote-exit:mac-browser-exit");
    assert_eq!(offer["service_kind"], "remote_exit");
    assert_eq!(offer["status"], "active");
    assert_eq!(offer["enabled"], true);
    assert_eq!(offer["grant_required"], false);

    let snapshot = home_services_snapshot(dir.path());
    let snapshot_text = snapshot.to_string();
    assert!(!snapshot_text.contains("connect_ticket"));
    assert!(!snapshot_text.contains("ticket:must-not-leak"));
    assert_eq!(snapshot["remote_offer_count"], 1);
    assert!(snapshot["remote_offers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|offer| offer["offer_id"] == "configured:remote-exit:mac-browser-exit"));

    let remove = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/services/offers")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"offer_id":"configured:remote-exit:mac-browser-exit","section":"others","selected":false}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = remove.status();
    let body = axum::body::to_bytes(remove.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&body).contains("managed by Exit Provider config"));
}

#[tokio::test]
async fn test_services_selection_state_is_principal_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(dir.path().join("config/exit-provider.json"), "{}").unwrap();
    let admin = passkey_authority_with_name(dir.path(), Some("alice"));
    let guest = passkey_authority_with_name_role(
        dir.path(),
        Some("bob"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let admin_token = app_token_for_authority(dir.path(), SERVICES_CAPSULE_ID, &admin);
    let guest_token = app_token_for_authority(dir.path(), SERVICES_CAPSULE_ID, &guest);

    let admin_enabled = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/services/offers")
                .header("x-elastos-home-token", admin_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"offer_id":"local:provider:browser-exit","section":"mine","selected":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admin_enabled.status(), StatusCode::OK);

    let guest_default = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", guest_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_default.status(), StatusCode::OK);
    let body = axum::body::to_bytes(guest_default.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["local_offer_count"], 0);

    let guest_saved_empty = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/services/offers")
                .header("x-elastos-home-token", guest_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"offer_id":"local:provider:browser-exit","section":"mine","selected":false}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_saved_empty.status(), StatusCode::OK);

    let admin_still_enabled = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", admin_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admin_still_enabled.status(), StatusCode::OK);
    let body = axum::body::to_bytes(admin_still_enabled.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["local_offer_count"], 1);
    assert!(!dir.path().join("config/services-state.json").exists());
}

#[tokio::test]
async fn test_services_remote_exit_request_delivers_provider_inbox_notification() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _left_runtime = start_fake_runtime(left.path(), bus.clone(), "services-left").await;
    let _right_runtime = start_fake_runtime(right.path(), bus, "services-right").await;
    let left_app = gateway_router(test_state(left.path()));
    let right_app = gateway_router(test_state(right.path()));
    let left_authority = passkey_authority_with_name(left.path(), Some("Alice"));
    let right_authority = passkey_authority_with_name(right.path(), Some("Bob"));
    let (_, left_did) = elastos_identity::load_or_create_did(left.path()).unwrap();
    let (_, right_did) = elastos_identity::load_or_create_did(right.path()).unwrap();

    for (app, token, body) in [
        (
            left_app.clone(),
            left_authority.system_token.as_str(),
            r#"{"handle":"Alice"}"#,
        ),
        (
            right_app.clone(),
            right_authority.system_token.as_str(),
            r#"{"handle":"Bob"}"#,
        ),
    ] {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/system/identity/profile-card")
                    .header("x-elastos-home-token", token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    std::fs::create_dir_all(right.path().join("config")).unwrap();
    std::fs::write(right.path().join("config/exit-provider.json"), "{}").unwrap();

    write_home_principal_object_json_for_authority(
        left.path(),
        &left_authority,
        "people-contacts.json",
        json!({
            "schema": "elastos.people.contacts-state/v1",
            "principal_id": left_authority.principal_id,
            "localhost_root": crate::auth::principal_localhost_root(&left_authority.principal_id),
            "updated_at": 10,
            "contacts": {
                "contact:right": {
                    "contact_id": "contact:right",
                    "peer_id": "services-right",
                    "did": right_did,
                    "display_name": "Bob",
                    "handle": "Bob",
                    "added_at": 10,
                    "updated_at": 10,
                    "source": "people_discovery"
                }
            }
        }),
    );
    write_home_principal_object_json_for_authority(
        right.path(),
        &right_authority,
        "people-contacts.json",
        json!({
            "schema": "elastos.people.contacts-state/v1",
            "principal_id": right_authority.principal_id,
            "localhost_root": crate::auth::principal_localhost_root(&right_authority.principal_id),
            "updated_at": 10,
            "contacts": {
                "contact:left": {
                    "contact_id": "contact:left",
                    "peer_id": "services-left",
                    "did": left_did,
                    "display_name": "Alice",
                    "handle": "Alice",
                    "added_at": 10,
                    "updated_at": 10,
                    "source": "people_discovery"
                }
            }
        }),
    );

    let right_services_token =
        app_token_for_authority(right.path(), SERVICES_CAPSULE_ID, &right_authority);
    let right_shared = right_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/services/offers")
                .header("x-elastos-home-token", right_services_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"offer_id":"local:provider:browser-exit","section":"mine","selected":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(right_shared.status(), StatusCode::OK);

    let left_services_token =
        app_token_for_authority(left.path(), SERVICES_CAPSULE_ID, &left_authority);
    let left_services = left_app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", left_services_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(left_services.status(), StatusCode::OK);
    let body = axum::body::to_bytes(left_services.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let offer_id = payload["available_remote_offers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|offer| offer["service_uri"] == "elastos://peer/browser-exit")
        .and_then(|offer| offer["offer_id"].as_str())
        .unwrap()
        .to_string();

    let requested = left_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/services/offers")
                .header("x-elastos-home-token", left_services_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "offer_id": offer_id,
                        "section": "others",
                        "selected": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = requested.status();
    let body = axum::body::to_bytes(requested.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let right_inbox_token =
        app_token_for_authority(right.path(), INBOX_CAPSULE_ID, &right_authority);
    let inbox = right_app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", right_inbox_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(inbox.into_body(), usize::MAX)
        .await
        .unwrap();
    let inbox_text = String::from_utf8_lossy(&body);
    assert!(!inbox_text.contains("connect_ticket"));
    assert!(!inbox_text.contains("ticket:"));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let notification = payload["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "service_access_request")
        .expect("provider inbox should contain a service access request");
    assert!(notification["title"]
        .as_str()
        .unwrap_or_default()
        .contains("Alice"));
    let action_id = notification["action_ref"]["action_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(action_id.starts_with("service-approve-request:"));

    let approved = right_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", right_inbox_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "action_id": action_id }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = approved.status();
    let body = axum::body::to_bytes(approved.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["message"]
        .as_str()
        .unwrap_or_default()
        .contains("private remote Exit grant was sent"));

    let inbox_after = right_app
        .oneshot(
            Request::builder()
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", right_inbox_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(inbox_after.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(!payload["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["kind"] == "service_access_request"));

    let left_summary_after = left_app
        .oneshot(
            Request::builder()
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", left_services_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(left_summary_after.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains("connect_ticket"));
    assert!(!text.contains("ticket:"));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let approved_offer = payload["remote_offers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|offer| offer["offer_id"] == offer_id)
        .expect("requester should keep the requested service selected");
    assert_eq!(approved_offer["status"], "active");
    assert_eq!(approved_offer["enabled"], true);
    assert_eq!(approved_offer["grant_required"], false);
    assert_eq!(
        approved_offer["grant_scope"],
        "installed_remote_carrier_exit_grant"
    );
    assert_eq!(approved_offer["route"], "/apps/browser/");
    let exit_config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(left.path().join("config/exit-provider.json")).unwrap(),
    )
    .unwrap();
    let installed_exit = exit_config["remote_carrier_exits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|exit| {
            exit["connect_ticket"] == "fake-ticket-services-right"
                && exit["peer_did"] == "services-right"
        })
        .expect("approval should install a private remote Carrier Exit grant");
    assert_eq!(
        installed_exit["allowed_principals"][0],
        left_authority.principal_id
    );
    assert_eq!(installed_exit["allowed_ports"], json!([80, 443]));
}

#[tokio::test]
async fn test_services_remote_exit_request_local_only_does_not_save_requested_state() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _left_runtime = start_fake_runtime(left.path(), bus.clone(), "services-left-local").await;
    let _right_runtime =
        start_fake_runtime(right.path(), bus.clone(), "services-right-local").await;
    let left_app = gateway_router(test_state(left.path()));
    let left_authority = passkey_authority_with_name(left.path(), Some("Alice"));
    let right_authority = passkey_authority_with_name(right.path(), Some("Bob"));
    let right_did = elastos_identity::load_or_create_did(right.path())
        .unwrap()
        .1;

    write_home_principal_object_json_for_authority(
        left.path(),
        &left_authority,
        "people-contacts.json",
        json!({
            "schema": "elastos.people.contacts-state/v1",
            "principal_id": left_authority.principal_id,
            "localhost_root": crate::auth::principal_localhost_root(&left_authority.principal_id),
            "updated_at": 10,
            "contacts": {
                "contact:right": {
                    "contact_id": "contact:right",
                    "peer_id": "services-right-local",
                    "did": right_did,
                    "display_name": "Bob",
                    "handle": "Bob",
                    "added_at": 10,
                    "updated_at": 10,
                    "source": "people_discovery"
                }
            }
        }),
    );
    write_home_principal_object_json_for_authority(
        right.path(),
        &right_authority,
        "people-contacts.json",
        json!({
            "schema": "elastos.people.contacts-state/v1",
            "principal_id": right_authority.principal_id,
            "localhost_root": crate::auth::principal_localhost_root(&right_authority.principal_id),
            "updated_at": 10,
            "contacts": {}
        }),
    );

    let left_services_token =
        app_token_for_authority(left.path(), SERVICES_CAPSULE_ID, &left_authority);
    let left_services = left_app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", left_services_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(left_services.status(), StatusCode::OK);
    let body = axum::body::to_bytes(left_services.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let offer_id = payload["available_remote_offers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|offer| offer["service_uri"] == "elastos://peer/browser-exit")
        .and_then(|offer| offer["offer_id"].as_str())
        .unwrap()
        .to_string();

    bus.lock()
        .await
        .local_only_message_substrings
        .push("service_access_request".to_string());
    let requested = left_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/services/offers")
                .header("x-elastos-home-token", left_services_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "offer_id": offer_id,
                        "section": "others",
                        "selected": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = requested.status();
    let body = axum::body::to_bytes(requested.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(String::from_utf8_lossy(&body).contains("not delivered"));

    let left_services_after = left_app
        .oneshot(
            Request::builder()
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", left_services_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(left_services_after.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["remote_offers"].as_array().unwrap().is_empty());
    assert!(payload["available_remote_offers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|offer| offer["offer_id"] == offer_id));
}

#[tokio::test]
async fn test_home_summary_reports_people_contacts_from_accepted_conversation_members() {
    let dir = tempfile::tempdir().unwrap();
    let guest = tempfile::tempdir().unwrap();
    let state = library_test_state(dir.path()).await;
    let app = gateway_router(state);
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/browser-engine-adapter.json"),
        serde_json::to_vec(&serde_json::json!({
            "adapters": [{
                "id": "browser-vm-product",
                "kind": "chromium_microvm",
                "network_mode": "runtime_net_only",
                "display_modes": ["webrtc_remote_display"],
                "supervisor": {
                    "program": "/tmp/browser-vm-engine-supervisor",
                    "control_socket_path": "/tmp/elastos-browser-vm-control-test.sock"
                }
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("bin")).unwrap();
    std::fs::write(dir.path().join("bin/ipfs-provider"), "").unwrap();
    let (_, owner_did) = elastos_identity::load_or_create_did(dir.path()).unwrap();
    let (_, guest_did) = elastos_identity::load_or_create_did(guest.path()).unwrap();

    crate::room_service::seed_room_owner(
        dir.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: owner_did.clone(),
            title: "Chat".to_string(),
        },
    )
    .unwrap();
    let invite = crate::room_service::export_room_invite_envelope(
        dir.path(),
        crate::room_service::RoomInviteInput {
            actor_did: owner_did.clone(),
            invited_did: guest_did.clone(),
            role: crate::room_service::RoomRole::Member,
        },
    )
    .unwrap();
    let imported = crate::room_service::import_room_invite_envelope(
        guest.path(),
        &serde_json::to_vec(&invite).unwrap(),
    )
    .unwrap();
    crate::room_service::accept_room_invite(
        guest.path(),
        crate::room_service::RoomInviteAcceptInput {
            actor_did: guest_did,
            invite_id: imported.invite_id.clone(),
        },
    )
    .unwrap();
    let acceptance = crate::room_service::export_room_acceptance_envelope_with_profile(
        guest.path(),
        &imported.invite_id,
        Some(crate::room_service::RoomProfileCardView {
            schema: "elastos.profile-card/v1".to_string(),
            profile_id: "profile:local:alice".to_string(),
            display_name: "Alice".to_string(),
            handle: Some("alice".to_string()),
            updated_at: 42,
        }),
    )
    .unwrap();
    crate::room_service::import_room_acceptance_envelope(
        dir.path(),
        &serde_json::to_vec(&acceptance).unwrap(),
    )
    .unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["people"]["schema"], "elastos.people.contacts/v1");
    assert_eq!(payload["people"]["contact_count"], 1);
    assert_eq!(payload["people"]["contacts"][0]["display_name"], "Alice");
    assert_eq!(payload["people"]["service_offer_count"], 2);
    let people_service_offers = payload["people"]["service_offers"].as_array().unwrap();
    assert_eq!(people_service_offers.len(), 2);
    assert_eq!(
        people_service_offers[0]["schema"],
        "elastos.service.offer/v1"
    );
    assert_eq!(
        people_service_offers[0]["service_uri"],
        "elastos://peer/conversation"
    );
    assert_eq!(people_service_offers[0]["enabled"], true);
    assert_eq!(people_service_offers[0]["capsule_hint"], "chat-room");
    assert!(people_service_offers.iter().any(|offer| {
        offer["service_uri"] == "elastos://peer/browser-exit"
            && offer["service_kind"] == "remote_exit"
            && offer["display_name"] == "Alice's Browser Exit"
            && offer["provider_uri"] == "elastos://exit/remote-carrier"
            && offer["status"] == "requestable"
            && offer["enabled"] == false
            && offer["grant_required"] == true
            && offer["grant_scope"] == "principal_scoped_remote_exit_grant"
            && offer["capsule_hint"] == "browser"
            && offer["route"].is_null()
    }));
    assert_eq!(payload["services"]["schema"], "elastos.runtime.services/v1");
    assert_eq!(
        payload["services"]["grant_model"],
        "principal_scoped_provider_grant"
    );
    assert_eq!(
        payload["services"]["capsule_contract"],
        "capsule -> runtime capability -> provider grant -> service"
    );
    assert_eq!(payload["services"]["remote_offer_count"], 0);
    assert_eq!(
        payload["services"]["remote_offers"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(payload["services"]["available_remote_offer_count"], 2);
    assert_eq!(
        payload["services"]["available_remote_offers"][0]["offer_id"],
        people_service_offers[0]["offer_id"]
    );
    assert!(payload["services"]["available_remote_offers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|offer| offer["offer_id"]
            .as_str()
            .unwrap_or_default()
            .ends_with(":browser-exit")
            && offer["service_uri"] == "elastos://peer/browser-exit"
            && offer["grant_required"] == true));
    let remote_browser_exit_offer_id = payload["services"]["available_remote_offers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|offer| offer["service_uri"] == "elastos://peer/browser-exit")
        .and_then(|offer| offer["offer_id"].as_str())
        .unwrap()
        .to_string();
    let grant_required_selection = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/services/offers")
                .header(
                    "x-elastos-home-token",
                    app_token_for_authority(dir.path(), SERVICES_CAPSULE_ID, &authority),
                )
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "offer_id": remote_browser_exit_offer_id,
                        "section": "others",
                        "selected": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = grant_required_selection.status();
    let body = axum::body::to_bytes(grant_required_selection.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(
        String::from_utf8_lossy(&body).contains("service access request"),
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(payload["services"]["local_offer_count"], 0);
    assert!(
        payload["services"]["available_local_offer_count"]
            .as_u64()
            .unwrap_or_default()
            >= 1
    );
    assert!(payload["services"]["available_local_offers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|offer| offer["service_kind"] == "conversation_host"
            && offer["grant_required"] == true
            && offer["provider_uri"] == "elastos://carrier/room"));
    assert!(payload["services"]["available_local_offers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|offer| offer["service_kind"] == "browser_engine"
            && offer["provider_uri"] == "elastos://browser-engine/*"
            && offer["runtime_contract"]["schema"] == "elastos.service.runtime-contract/v1"
            && offer["runtime_contract"]["backing_substrate"] == "local_microvm"
            && offer["runtime_contract"]["supported_display_modes"]
                .as_array()
                .unwrap()
                .len()
                == 1
            && offer["runtime_contract"]["supported_display_modes"][0] == "webrtc_remote_display"
            && offer["runtime_contract"]["supported_guarantee_levels"][0] == "mechanism_microvm"
            && offer["runtime_contract"]["direct_network"] == false
            && offer["runtime_contract"]["wallet_injection"] == false
            && offer["route"] == "/apps/browser/"));
    assert!(payload["services"]["available_local_offers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|offer| offer["service_kind"] == "content_availability"
            && offer["provider_uri"] == "elastos://content/*"
            && offer["grant_required"] == true));
    assert_eq!(
        payload["people"]["contacts"][0]["profile_card"]["display_name"],
        "Alice"
    );
    assert_eq!(
        payload["people"]["contacts"][0]["route"],
        "/apps/chat-room/"
    );
    assert!(payload["people"]["contacts"][0]["contact_id"]
        .as_str()
        .unwrap()
        .starts_with("contact:"));
}

#[tokio::test]
async fn test_people_invite_create_returns_conversation_join_link() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "people-invite-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/invites/create")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header("host", "localhost:61180")
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
    let invite_url = payload["invite_url"].as_str().unwrap_or_default();
    assert!(invite_url.starts_with("elastos://peer/invite?token="));
    assert_eq!(payload["issuer_gateway"], "http://localhost:61180");
    assert_eq!(payload["room_title"], "Chat");
}

#[tokio::test]
async fn test_people_discovery_toggle_persists_in_home_summary() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let token = home_app_token(dir.path());

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery")
                .header("x-elastos-home-token", token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
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
    assert_eq!(payload["schema"], "elastos.people.discovery/v1");
    assert_eq!(payload["enabled"], true);
    assert_eq!(payload["visibility"], "everyone");
    assert_eq!(payload["status"], "runtime_unavailable");
    let expires_at = payload["expires_at"].as_u64().unwrap();
    let remaining = payload["remaining_seconds"].as_u64().unwrap();
    assert!(expires_at > crate::auth::now_ts());
    assert!(remaining > 0 && remaining <= 600);

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", token.as_str())
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
    assert_eq!(payload["people"]["discovery"]["enabled"], true);
    assert_eq!(payload["people"]["discovery"]["visibility"], "everyone");
    assert_eq!(payload["people"]["discovery"]["expires_at"], expires_at);
    assert!(
        payload["people"]["discovery"]["remaining_seconds"]
            .as_u64()
            .unwrap()
            <= 600
    );
}

#[tokio::test]
async fn test_people_discovery_expired_visibility_reports_off_and_refresh_does_not_publish() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let runtime = start_fake_runtime(dir.path(), bus, "people-expired").await;
    let app = gateway_router(test_state(dir.path()));
    let token = home_app_token(dir.path());
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let localhost_root = crate::auth::principal_localhost_root(&context.principal_id);
    write_home_principal_object_json(
        dir.path(),
        "people-discovery.json",
        json!({
            "schema": "elastos.people.discovery-state/v1",
            "principal_id": context.principal_id,
            "localhost_root": localhost_root,
            "enabled": true,
            "enabled_until": crate::auth::now_ts().saturating_sub(1),
            "updated_at": crate::auth::now_ts().saturating_sub(2),
            "local_peer_id": "people-expired",
            "peers": {},
            "requests": {}
        }),
    );

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", token.as_str())
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
    assert_eq!(payload["people"]["discovery"]["enabled"], false);
    assert_eq!(payload["people"]["discovery"]["visibility"], "off");
    assert_eq!(payload["people"]["discovery"]["status"], "off");
    assert_eq!(
        payload["people"]["discovery"]["expires_at"],
        serde_json::Value::Null
    );

    let refresh = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refresh.status(), StatusCode::OK);
    let body = axum::body::to_bytes(refresh.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["enabled"], false);
    assert_eq!(payload["visibility"], "off");
    let requests = runtime.provider_requests.lock().await.clone();
    assert!(
        !requests
            .iter()
            .any(|request| { request["scheme"] == "peer" && request["op"] == "gossip_send" }),
        "expired discovery refresh must not publish presence"
    );
}

#[tokio::test]
async fn test_people_discovery_refresh_finds_visible_peer() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _left_runtime = start_fake_runtime(left.path(), bus.clone(), "people-left").await;
    let _right_runtime = start_fake_runtime(right.path(), bus, "people-right").await;
    let left_app = gateway_router(test_state(left.path()));
    let right_app = gateway_router(test_state(right.path()));
    let left_token = home_app_token(left.path());
    let right_token = home_app_token(right.path());

    let left_enable = left_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery")
                .header("x-elastos-home-token", left_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(left_enable.status(), StatusCode::OK);

    let right_enable = right_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery")
                .header("x-elastos-home-token", right_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(right_enable.status(), StatusCode::OK);

    let left_refresh = left_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", left_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = left_refresh.status();
    let body = axum::body::to_bytes(left_refresh.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["status"], "visible");
    assert_eq!(payload["local_peer_id"], "people-left");
    assert!(payload["discovered_peers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|peer| peer["peer_id"] == "people-right"));
}

#[tokio::test]
async fn test_people_discovery_refresh_waits_for_pending_peer_capability() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime =
        start_fake_runtime_with_pending_capabilities(dir.path(), bus, "people-pending").await;
    let app = gateway_router(test_state(dir.path()));
    let token = home_app_token(dir.path());

    let enable = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery")
                .header("x-elastos-home-token", token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(enable.status(), StatusCode::OK);

    let refresh = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = refresh.status();
    let body = axum::body::to_bytes(refresh.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["status"], "visible");
    assert_eq!(payload["local_peer_id"], "people-pending");
}

#[tokio::test]
async fn test_people_discovery_refresh_joins_configured_peer_ticket() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let runtime = start_fake_runtime(dir.path(), bus, "people-configured").await;
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("people-discovery-peers.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": "elastos.people.discovery-peers/v1",
            "peers": [
                { "connect_ticket": "fake-configured-ticket" }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let app = gateway_router(test_state(dir.path()));
    let token = home_app_token(dir.path());

    let enable = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery")
                .header("x-elastos-home-token", token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(enable.status(), StatusCode::OK);

    let refresh = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = refresh.status();
    let body = axum::body::to_bytes(refresh.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let requests = runtime.provider_requests.lock().await.clone();
    assert!(requests.iter().any(|request| {
        request["scheme"] == "peer"
            && request["op"] == "connect"
            && request["body"]["ticket"] == "fake-configured-ticket"
    }));
    assert!(requests.iter().any(|request| {
        request["scheme"] == "peer"
            && request["op"] == "gossip_join_peers"
            && request["body"]["topic"] == "__elastos_internal/people-discovery-v1"
            && request["body"]["peers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|peer| peer == "trusted-source-peer")
    }));
}

#[tokio::test]
async fn test_people_discovery_refresh_reuses_recent_join_and_presence() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let runtime = start_fake_runtime(dir.path(), bus, "people-quiet").await;
    let app = gateway_router(test_state(dir.path()));
    let token = home_app_token(dir.path());

    let enable = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery")
                .header("x-elastos-home-token", token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(enable.status(), StatusCode::OK);
    runtime.provider_requests.lock().await.clear();

    let refresh = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = refresh.status();
    let body = axum::body::to_bytes(refresh.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["changed"], false);
    assert!(payload["refresh_fingerprint"].as_str().unwrap_or("").len() >= 32);
    assert!(payload["next_refresh_after_ms"].as_u64().unwrap_or(0) >= 3_000);

    let requests = runtime.provider_requests.lock().await.clone();
    assert!(requests
        .iter()
        .any(|request| { request["scheme"] == "peer" && request["op"] == "gossip_recv" }));
    assert!(
        requests.iter().all(|request| {
            request["op"] != "connect"
                && request["op"] != "gossip_join"
                && request["op"] != "gossip_join_peers"
                && request["op"] != "gossip_send"
        }),
        "recent background refresh should receive only, not redo Carrier bootstrap/presence: {requests:#?}"
    );
}

fn read_home_principal_object_json(
    data_dir: &std::path::Path,
    filename: &str,
) -> serde_json::Value {
    let context = local_home_launch_token_context(data_dir).unwrap();
    let localhost_root = crate::auth::principal_localhost_root(&context.principal_id);
    let uri = format!("{localhost_root}/.AppData/ElastOS/Home/{filename}");
    let path = elastos_common::localhost::rooted_localhost_fs_path(data_dir, &uri).unwrap();
    if !path.is_file() {
        return json!({});
    }
    let bytes = crate::auth::read_principal_root_object(
        data_dir,
        &context.principal_id,
        &localhost_root,
        &uri,
        &path,
    )
    .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn write_home_principal_object_json(
    data_dir: &std::path::Path,
    filename: &str,
    value: serde_json::Value,
) {
    let context = local_home_launch_token_context(data_dir).unwrap();
    let localhost_root = crate::auth::principal_localhost_root(&context.principal_id);
    let uri = format!("{localhost_root}/.AppData/ElastOS/Home/{filename}");
    let path = elastos_common::localhost::rooted_localhost_fs_path(data_dir, &uri).unwrap();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let bytes = serde_json::to_vec_pretty(&value).unwrap();
    crate::auth::write_principal_root_object(
        data_dir,
        &context.principal_id,
        &localhost_root,
        &uri,
        &path,
        &bytes,
    )
    .unwrap();
}

fn write_home_principal_object_json_for_authority(
    data_dir: &std::path::Path,
    authority: &TestPasskeyAuthority,
    filename: &str,
    value: serde_json::Value,
) {
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{localhost_root}/.AppData/ElastOS/Home/{filename}");
    let path = elastos_common::localhost::rooted_localhost_fs_path(data_dir, &uri).unwrap();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let bytes = serde_json::to_vec_pretty(&value).unwrap();
    crate::auth::write_principal_root_object(
        data_dir,
        &authority.principal_id,
        &localhost_root,
        &uri,
        &path,
        &bytes,
    )
    .unwrap();
}

#[tokio::test]
async fn test_people_discovery_request_send_failure_does_not_save_requested_state() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _left_runtime =
        start_fake_runtime(left.path(), bus.clone(), "people-left-request-fail").await;
    let _right_runtime =
        start_fake_runtime(right.path(), bus.clone(), "people-right-request-fail").await;
    let left_app = gateway_router(test_state(left.path()));
    let right_app = gateway_router(test_state(right.path()));
    let left_token = home_app_token(left.path());
    let right_token = home_app_token(right.path());

    for (app, token) in [
        (left_app.clone(), left_token.as_str()),
        (right_app.clone(), right_token.as_str()),
    ] {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/people/discovery")
                    .header("x-elastos-home-token", token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"enabled":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let left_refresh = left_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", left_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(left_refresh.status(), StatusCode::OK);

    bus.lock()
        .await
        .fail_message_substrings
        .push(r#""kind":"request""#.to_string());
    let request = left_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/requests")
                .header("x-elastos-home-token", left_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"peer_id":"people-right-request-fail"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = request.status();
    let body = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(String::from_utf8_lossy(&body).contains("people discovery delivery failed"));

    let state = read_home_principal_object_json(left.path(), "people-discovery.json");
    assert!(state["requests"].as_object().unwrap().is_empty());
}

#[tokio::test]
async fn test_people_discovery_accept_send_failure_does_not_save_joined_state() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _left_runtime =
        start_fake_runtime(left.path(), bus.clone(), "people-left-accept-fail").await;
    let _right_runtime =
        start_fake_runtime(right.path(), bus.clone(), "people-right-accept-fail").await;
    let left_app = gateway_router(test_state(left.path()));
    let right_app = gateway_router(test_state(right.path()));
    let left_token = home_app_token(left.path());
    let right_token = home_app_token(right.path());

    for (app, token) in [
        (left_app.clone(), left_token.as_str()),
        (right_app.clone(), right_token.as_str()),
    ] {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/people/discovery")
                    .header("x-elastos-home-token", token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"enabled":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let left_refresh = left_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", left_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(left_refresh.status(), StatusCode::OK);

    let left_request = left_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/requests")
                .header("x-elastos-home-token", left_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"peer_id":"people-right-accept-fail"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(left_request.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let request_id = payload["requests"][0]["request_id"]
        .as_str()
        .unwrap()
        .to_string();

    let right_refresh = right_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", right_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = right_refresh.status();
    let body = axum::body::to_bytes(right_refresh.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["requests"][0]["status"], "incoming");

    bus.lock()
        .await
        .fail_message_substrings
        .push(r#""kind":"acceptance""#.to_string());
    let right_accept = right_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/apps/people/discovery/requests/{}/accept",
                    request_id
                ))
                .header("x-elastos-home-token", right_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = right_accept.status();
    let body = axum::body::to_bytes(right_accept.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        String::from_utf8_lossy(&body)
    );

    let state = read_home_principal_object_json(right.path(), "people-discovery.json");
    assert_eq!(
        state["requests"][&request_id]["status"].as_str(),
        Some("incoming")
    );
    let contacts = read_home_principal_object_json(right.path(), "people-contacts.json");
    assert!(contacts["contacts"]
        .as_object()
        .map(|contacts| contacts.is_empty())
        .unwrap_or(true));
}

#[tokio::test]
async fn test_people_discovery_join_send_failure_does_not_save_joined_state() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _left_runtime = start_fake_runtime(left.path(), bus.clone(), "people-left-join-fail").await;
    let left_app = gateway_router(test_state(left.path()));
    let left_token = home_app_token(left.path());
    let (_, left_did) = elastos_identity::load_or_create_did(left.path()).unwrap();
    let (_, right_did) = elastos_identity::load_or_create_did(right.path()).unwrap();
    crate::room_service::seed_room_owner(
        right.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: right_did.clone(),
            title: "Chat".to_string(),
        },
    )
    .unwrap();
    let invite = crate::room_service::export_room_invite_envelope(
        right.path(),
        crate::room_service::RoomInviteInput {
            actor_did: right_did.clone(),
            invited_did: left_did,
            role: crate::room_service::RoomRole::Member,
        },
    )
    .unwrap();
    let imported = crate::room_service::import_room_invite_envelope(
        left.path(),
        &serde_json::to_vec(&invite).unwrap(),
    )
    .unwrap();
    let request_id = "request:join-send-fails".to_string();
    let context = local_home_launch_token_context(left.path()).unwrap();
    let localhost_root = crate::auth::principal_localhost_root(&context.principal_id);
    write_home_principal_object_json(
        left.path(),
        "people-discovery.json",
        json!({
            "schema": "elastos.people.discovery-state/v1",
            "principal_id": context.principal_id,
            "localhost_root": localhost_root,
            "enabled": true,
            "updated_at": 42,
            "local_peer_id": "people-left-join-fail",
            "peers": {},
            "requests": {
                request_id.clone(): {
                    "request_id": request_id.clone(),
                    "peer_id": "people-right-join-fail",
                    "did": imported.invited_by,
                    "display_name": "Right",
                    "handle": null,
                    "created_at": 42,
                    "status": "accepted",
                    "invite_id": imported.invite_id,
                }
            }
        }),
    );

    bus.lock()
        .await
        .fail_message_substrings
        .push(request_id.clone());
    let join = left_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/apps/people/discovery/requests/{}/join",
                    request_id
                ))
                .header("x-elastos-home-token", left_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = join.status();
    let body = axum::body::to_bytes(join.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        String::from_utf8_lossy(&body)
    );

    let state = read_home_principal_object_json(left.path(), "people-discovery.json");
    assert_eq!(
        state["requests"][&request_id]["status"].as_str(),
        Some("accepted")
    );
}

#[tokio::test]
async fn test_people_discovery_request_accept_without_passkey_handle() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _left_runtime = start_fake_runtime(left.path(), bus.clone(), "people-left-no-handle").await;
    let _right_runtime = start_fake_runtime(right.path(), bus, "people-right-no-handle").await;
    let left_app = gateway_router(test_state(left.path()));
    let right_app = gateway_router(test_state(right.path()));
    let left_token = home_app_token(left.path());
    let right_token = home_app_token(right.path());
    let (_, left_did) = elastos_identity::load_or_create_did(left.path()).unwrap();

    for (app, token) in [
        (left_app.clone(), left_token.as_str()),
        (right_app.clone(), right_token.as_str()),
    ] {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/people/discovery")
                    .header("x-elastos-home-token", token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"enabled":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let left_refresh = left_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", left_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(left_refresh.status(), StatusCode::OK);

    let left_request = left_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/requests")
                .header("x-elastos-home-token", left_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"peer_id":"people-right-no-handle"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(left_request.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let request_id = payload["requests"][0]["request_id"]
        .as_str()
        .unwrap()
        .to_string();

    let right_refresh = right_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", right_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = right_refresh.status();
    let body = axum::body::to_bytes(right_refresh.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["requests"][0]["status"], "incoming");
    assert_eq!(payload["requests"][0]["did"], left_did);

    let right_accept = right_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/apps/people/discovery/requests/{}/accept",
                    request_id
                ))
                .header("x-elastos-home-token", right_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = right_accept.status();
    let body = axum::body::to_bytes(right_accept.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["request_count"], 0);
    assert!(payload["requests"].as_array().unwrap().is_empty());

    let right_summary = right_app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", right_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(right_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["people"]["contacts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|contact| {
            contact["relationship"] == "connected"
                && contact["device_label"] == "people-left-no-handle"
        }));

    let left_refresh = left_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", left_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = left_refresh.status();
    let body = axum::body::to_bytes(left_refresh.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["request_count"], 0);
    assert!(payload["requests"].as_array().unwrap().is_empty());

    let left_summary = left_app
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", left_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(left_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["people"]["contacts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|contact| {
            contact["relationship"] == "connected"
                && contact["device_label"] == "people-right-no-handle"
        }));
}

#[tokio::test]
async fn test_people_discovery_request_accept_is_idempotent_for_active_member() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _left_runtime =
        start_fake_runtime(left.path(), bus.clone(), "people-left-active-member").await;
    let _right_runtime = start_fake_runtime(right.path(), bus, "people-right-active-member").await;
    let left_app = gateway_router(test_state(left.path()));
    let right_app = gateway_router(test_state(right.path()));
    let left_token = home_app_token(left.path());
    let right_token = home_app_token(right.path());
    let (_, left_did) = elastos_identity::load_or_create_did(left.path()).unwrap();
    let (_, right_did) = elastos_identity::load_or_create_did(right.path()).unwrap();

    crate::room_service::seed_room_owner(
        right.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: right_did.clone(),
            title: "Chat".to_string(),
        },
    )
    .unwrap();
    let invite = crate::room_service::export_room_invite_envelope(
        right.path(),
        crate::room_service::RoomInviteInput {
            actor_did: right_did,
            invited_did: left_did.clone(),
            role: crate::room_service::RoomRole::Member,
        },
    )
    .unwrap();
    crate::room_service::accept_room_invite(
        right.path(),
        crate::room_service::RoomInviteAcceptInput {
            actor_did: left_did.clone(),
            invite_id: invite.payload.invite_id,
        },
    )
    .unwrap();

    for (app, token) in [
        (left_app.clone(), left_token.as_str()),
        (right_app.clone(), right_token.as_str()),
    ] {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/people/discovery")
                    .header("x-elastos-home-token", token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"enabled":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let left_refresh = left_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", left_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(left_refresh.status(), StatusCode::OK);

    let left_request = left_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/requests")
                .header("x-elastos-home-token", left_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"peer_id":"people-right-active-member"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(left_request.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let request_id = payload["requests"][0]["request_id"]
        .as_str()
        .unwrap()
        .to_string();

    let right_refresh = right_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", right_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = right_refresh.status();
    let body = axum::body::to_bytes(right_refresh.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["requests"][0]["status"], "incoming");
    assert_eq!(payload["requests"][0]["did"], left_did);

    let right_accept = right_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/apps/people/discovery/requests/{}/accept",
                    request_id
                ))
                .header("x-elastos-home-token", right_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = right_accept.status();
    let body = axum::body::to_bytes(right_accept.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["request_count"], 0);
    assert!(payload["requests"].as_array().unwrap().is_empty());

    let repeated_accept = right_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/apps/people/discovery/requests/{}/accept",
                    request_id
                ))
                .header("x-elastos-home-token", right_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = repeated_accept.status();
    let body = axum::body::to_bytes(repeated_accept.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["request_count"], 0);
    assert!(payload["requests"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_people_discovery_request_accept_contact_round_trip() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _left_runtime = start_fake_runtime(left.path(), bus.clone(), "people-left").await;
    let _right_runtime = start_fake_runtime(right.path(), bus, "people-right").await;
    let left_app = gateway_router(test_state(left.path()));
    let right_app = gateway_router(test_state(right.path()));
    let left_authority = passkey_authority_with_name(left.path(), Some("Alice"));
    let right_authority = passkey_authority_with_name(right.path(), Some("Bob"));
    let (_, left_did) = elastos_identity::load_or_create_did(left.path()).unwrap();

    for (app, token, body) in [
        (
            left_app.clone(),
            left_authority.system_token.as_str(),
            r#"{"handle":"Alice"}"#,
        ),
        (
            right_app.clone(),
            right_authority.system_token.as_str(),
            r#"{"handle":"Bob"}"#,
        ),
    ] {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/system/identity/profile-card")
                    .header("x-elastos-home-token", token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    for (app, token) in [
        (left_app.clone(), left_authority.home_token.as_str()),
        (right_app.clone(), right_authority.home_token.as_str()),
    ] {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/people/discovery")
                    .header("x-elastos-home-token", token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"enabled":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let left_refresh = left_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", left_authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(left_refresh.status(), StatusCode::OK);

    let left_request = left_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/requests")
                .header("x-elastos-home-token", left_authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"peer_id":"people-right"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = left_request.status();
    let body = axum::body::to_bytes(left_request.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let request_id = payload["requests"][0]["request_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(payload["requests"][0]["status"], "requested");

    let right_refresh = right_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", right_authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = right_refresh.status();
    let body = axum::body::to_bytes(right_refresh.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["requests"][0]["request_id"], request_id);
    assert_eq!(payload["requests"][0]["status"], "incoming");
    assert_eq!(payload["requests"][0]["did"], left_did);

    let right_accept = right_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/apps/people/discovery/requests/{}/accept",
                    request_id
                ))
                .header("x-elastos-home-token", right_authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = right_accept.status();
    let body = axum::body::to_bytes(right_accept.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["request_count"], 0);
    assert!(payload["requests"].as_array().unwrap().is_empty());

    let right_summary = right_app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", right_authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(right_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["people"]["contacts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|contact| {
            contact["display_name"] == "Alice"
                && contact["relationship"] == "connected"
                && contact["device_label"] == "people-left"
        }));

    let left_refresh = left_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", left_authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = left_refresh.status();
    let body = axum::body::to_bytes(left_refresh.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["request_count"], 0);
    assert!(payload["requests"].as_array().unwrap().is_empty());
    assert!(!payload["discovered_peers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|peer| peer["peer_id"] == "people-right"));

    let left_summary = left_app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", left_authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(left_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["people"]["contacts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|contact| {
            contact["display_name"] == "Bob"
                && contact["relationship"] == "connected"
                && contact["device_label"] == "people-right"
        }));

    let right_refresh = right_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/discovery/refresh")
                .header("x-elastos-home-token", right_authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = right_refresh.status();
    let body = axum::body::to_bytes(right_refresh.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["request_count"], 0);
    assert!(payload["requests"].as_array().unwrap().is_empty());
    assert!(!payload["discovered_peers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|peer| peer["peer_id"] == "people-left"));

    let right_summary = right_app
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", right_authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(right_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["people"]["contacts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|contact| contact["display_name"] == "Alice"));
}

#[tokio::test]
async fn test_people_invite_create_allows_conversation_member_when_user_invites_are_open() {
    let owner = tempfile::tempdir().unwrap();
    let member = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(member.path(), bus, "people-member-invite-peer").await;
    let app = gateway_router(test_state(member.path()));
    let authority = passkey_authority_with_name(member.path(), Some("alice"));
    let (_, owner_did) = elastos_identity::load_or_create_did(owner.path()).unwrap();
    let (_, member_did) = elastos_identity::load_or_create_did(member.path()).unwrap();

    crate::room_service::seed_room_owner(
        owner.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: owner_did.clone(),
            title: "Chat".to_string(),
        },
    )
    .unwrap();
    let invite = crate::room_service::export_room_invite_envelope(
        owner.path(),
        crate::room_service::RoomInviteInput {
            actor_did: owner_did,
            invited_did: member_did.clone(),
            role: crate::room_service::RoomRole::Member,
        },
    )
    .unwrap();
    let imported = crate::room_service::import_room_invite_envelope(
        member.path(),
        &serde_json::to_vec(&invite).unwrap(),
    )
    .unwrap();
    crate::room_service::accept_room_invite(
        member.path(),
        crate::room_service::RoomInviteAcceptInput {
            actor_did: member_did,
            invite_id: imported.invite_id,
        },
    )
    .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/invites/create")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header("host", "localhost:61180")
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
    assert!(payload["invite_url"]
        .as_str()
        .unwrap_or_default()
        .starts_with("elastos://peer/invite?token="));
    assert_eq!(payload["issuer_gateway"], "http://localhost:61180");
}

#[tokio::test]
async fn test_people_contact_remove_hides_accepted_conversation_contact_locally() {
    let dir = tempfile::tempdir().unwrap();
    let guest = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "people-remove-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
    let (_, owner_did) = elastos_identity::load_or_create_did(dir.path()).unwrap();
    let (_, guest_did) = elastos_identity::load_or_create_did(guest.path()).unwrap();

    crate::room_service::seed_room_owner(
        dir.path(),
        crate::room_service::RoomOwnerSeedInput {
            owner_did: owner_did.clone(),
            title: "Chat".to_string(),
        },
    )
    .unwrap();
    let invite = crate::room_service::export_room_invite_envelope(
        dir.path(),
        crate::room_service::RoomInviteInput {
            actor_did: owner_did.clone(),
            invited_did: guest_did.clone(),
            role: crate::room_service::RoomRole::Member,
        },
    )
    .unwrap();
    let imported = crate::room_service::import_room_invite_envelope(
        guest.path(),
        &serde_json::to_vec(&invite).unwrap(),
    )
    .unwrap();
    crate::room_service::accept_room_invite(
        guest.path(),
        crate::room_service::RoomInviteAcceptInput {
            actor_did: guest_did,
            invite_id: imported.invite_id.clone(),
        },
    )
    .unwrap();
    let acceptance =
        crate::room_service::export_room_acceptance_envelope(guest.path(), &imported.invite_id)
            .unwrap();
    crate::room_service::import_room_acceptance_envelope(
        dir.path(),
        &serde_json::to_vec(&acceptance).unwrap(),
    )
    .unwrap();
    let contact_id = home_people_contact_id(&invite.payload.invited_did);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/contacts/remove")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({ "contact_id": contact_id }).to_string(),
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
    assert_eq!(payload["schema"], "elastos.people.contact-remove/v1");
    assert_eq!(payload["scope"], "local_people");

    let room_summary = crate::room_service::load_summary(dir.path()).unwrap();
    assert_eq!(
        room_summary
            .room_control
            .members
            .iter()
            .filter(|member| member.member_did != owner_did)
            .count(),
        1,
        "People removal must not eject members from the conversation"
    );

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.as_str())
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
    assert_eq!(payload["people"]["contact_count"], 0);
    assert!(payload["people"]["contacts"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_home_events_long_poll_returns_cursor_and_keepalive() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("events"));

    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/events?wait_ms=0")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
    assert_eq!(first_json["schema"], "elastos.home.events/v1");
    assert_eq!(first_json["keepalive"], false);
    assert!(first_json["cursor"].as_str().unwrap().starts_with("v1:"));
    assert!(first_json["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["kind"] == "home.summary.changed"));

    let cursor = first_json["cursor"].as_str().unwrap();
    let second = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/apps/home/events?wait_ms=0&cursor={cursor}"))
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second_body = axum::body::to_bytes(second.into_body(), usize::MAX)
        .await
        .unwrap();
    let second_json: serde_json::Value = serde_json::from_slice(&second_body).unwrap();
    assert_eq!(second_json["schema"], "elastos.home.events/v1");
    assert_eq!(second_json["cursor"], cursor);
    assert_eq!(second_json["keepalive"], true);
    assert!(second_json["events"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_home_events_stream_requires_home_authority_and_serves_sse() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("events"));

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/events/stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::FORBIDDEN);

    let authorized = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/events/stream")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorized.status(), StatusCode::OK);
    assert!(
        authorized
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream")),
        "Home event stream should be served as SSE"
    );
    assert_eq!(
        authorized
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-cache, no-transform"),
        "Home SSE must not be cached or transformed by proxies"
    );
    assert_eq!(
        authorized
            .headers()
            .get("x-accel-buffering")
            .and_then(|value| value.to_str().ok()),
        Some("no"),
        "nginx must not buffer realtime Home events"
    );
}

#[tokio::test]
async fn test_home_summary_and_events_include_browser_wallet_approvals() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
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

    let before = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/events?wait_ms=0")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(before.status(), StatusCode::OK);
    let before_body = axum::body::to_bytes(before.into_body(), usize::MAX)
        .await
        .unwrap();
    let before_json: serde_json::Value = serde_json::from_slice(&before_body).unwrap();
    let cursor = before_json["cursor"].as_str().unwrap().to_string();

    let request = app
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
                        "page_url":"https://ela.city/",
                        "origin":"https://ela.city"
                    }}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(request.status(), StatusCode::OK);

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_json: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(summary_json["notifications"]["attention_count"], 1);
    assert_eq!(
        summary_json["notifications"]["entries"][0]["title"],
        "Transaction approval request"
    );

    let events = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/apps/home/events?wait_ms=0&cursor={cursor}"))
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(events.status(), StatusCode::OK);
    let events_body = axum::body::to_bytes(events.into_body(), usize::MAX)
        .await
        .unwrap();
    let events_json: serde_json::Value = serde_json::from_slice(&events_body).unwrap();
    assert!(events_json["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| { event["kind"] == "wallet.requests.changed" && event["scope"] == "wallet" }));
}

#[tokio::test]
async fn test_system_updates_home_background_image() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let admin = passkey_authority_with_name(dir.path(), Some("admin"));
    let guest = passkey_authority_with_name_role(
        dir.path(),
        Some("guest"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let admin_protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &admin.principal_id);
    let guest_protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &guest.principal_id);

    let updated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/appearance/background-image")
                .header("x-elastos-home-token", admin.system_token.clone())
                .header(CONTENT_TYPE, "image/png")
                .body(Body::from("admin-image"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated_body = axum::body::to_bytes(updated.into_body(), usize::MAX)
        .await
        .unwrap();
    let updated_payload: serde_json::Value = serde_json::from_slice(&updated_body).unwrap();
    let background_url = updated_payload["background_image_url"]
        .as_str()
        .expect("background url");
    assert!(
        background_url.starts_with("/api/apps/home/appearance/background-image?scope="),
        "{background_url}"
    );
    assert!(background_url.contains("&v="), "{background_url}");

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", admin.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_payload: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(
        summary_payload["appearance"]["background_image_url"],
        updated_payload["background_image_url"]
    );
    assert_eq!(
        summary_payload["appearance"]["background_overlay_enabled"],
        serde_json::Value::Bool(false)
    );
    assert_eq!(
        summary_payload["appearance"]["background_overlay_opacity"],
        serde_json::json!(HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT)
    );

    let overlay = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/appearance/background-overlay")
                .header("x-elastos-home-token", admin.system_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true,"opacity":0.42}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(overlay.status(), StatusCode::OK);
    let overlay_body = axum::body::to_bytes(overlay.into_body(), usize::MAX)
        .await
        .unwrap();
    let overlay_payload: serde_json::Value = serde_json::from_slice(&overlay_body).unwrap();
    assert_eq!(
        overlay_payload["background_overlay_enabled"],
        serde_json::Value::Bool(true)
    );
    assert_eq!(
        overlay_payload["background_overlay_opacity"],
        serde_json::json!(0.42)
    );

    let guest_summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", guest.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_summary.status(), StatusCode::OK);
    let guest_summary_body = axum::body::to_bytes(guest_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let guest_summary_payload: serde_json::Value =
        serde_json::from_slice(&guest_summary_body).unwrap();
    assert!(guest_summary_payload["appearance"]["background_image_url"].is_null());
    assert_eq!(
        guest_summary_payload["appearance"]["background_overlay_enabled"],
        serde_json::Value::Bool(false)
    );

    let guest_updated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/appearance/background-image")
                .header("x-elastos-home-token", guest.system_token.clone())
                .header(CONTENT_TYPE, "image/jpeg")
                .body(Body::from("guest-image"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_updated.status(), StatusCode::OK);

    let image = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/appearance/background-image")
                .header("x-elastos-home-token", admin.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(image.status(), StatusCode::OK);
    assert_eq!(
        image
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );
    let image_body = axum::body::to_bytes(image.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&image_body[..], b"admin-image");

    let guest_image = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/appearance/background-image")
                .header("x-elastos-home-token", guest.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_image.status(), StatusCode::OK);
    assert_eq!(
        guest_image
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/jpeg")
    );
    let guest_image_body = axum::body::to_bytes(guest_image.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&guest_image_body[..], b"guest-image");

    let admin_path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!(
            "{}/.AppData/ElastOS/Home/Appearance/background-image.png",
            admin_protection.localhost_root
        ),
    )
    .unwrap();
    let admin_stored = std::fs::read_to_string(&admin_path).unwrap();
    assert!(!admin_stored.contains("admin-image"));
    assert!(admin_stored.contains("elastos.principal-root.object/v1"));
    assert!(admin_stored.contains(&admin_protection.localhost_root));

    let guest_path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!(
            "{}/.AppData/ElastOS/Home/Appearance/background-image.jpg",
            guest_protection.localhost_root
        ),
    )
    .unwrap();
    let guest_stored = std::fs::read_to_string(&guest_path).unwrap();
    assert!(!guest_stored.contains("guest-image"));
    assert!(guest_stored.contains("elastos.principal-root.object/v1"));
    assert!(guest_stored.contains(&guest_protection.localhost_root));

    let oversized = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/appearance/background-image")
                .header("x-elastos-home-token", admin.system_token.clone())
                .header(CONTENT_TYPE, "image/png")
                .body(Body::from(vec![0_u8; HOME_BACKGROUND_IMAGE_MAX_BYTES + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::BAD_REQUEST);
    let oversized_body = axum::body::to_bytes(oversized.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        std::str::from_utf8(&oversized_body).unwrap(),
        "background image is larger than 5 MB"
    );

    let reset = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/apps/system/appearance/background-image")
                .header("x-elastos-home-token", admin.system_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reset.status(), StatusCode::OK);
    let reset_body = axum::body::to_bytes(reset.into_body(), usize::MAX)
        .await
        .unwrap();
    let reset_payload: serde_json::Value = serde_json::from_slice(&reset_body).unwrap();
    assert!(reset_payload["background_image_url"].is_null());

    let missing_image = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/appearance/background-image")
                .header("x-elastos-home-token", admin.home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_image.status(), StatusCode::NOT_FOUND);

    let guest_image_after_admin_reset = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/appearance/background-image")
                .header("x-elastos-home-token", guest.home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_image_after_admin_reset.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_home_runtime_ensure_reuses_running_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "home-peer").await;

    let app = gateway_router(test_state(dir.path()));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/runtime/ensure")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["running"], true);
    assert_eq!(payload["version"], env!("ELASTOS_VERSION"));
    assert!(payload["note"].is_null());
    assert_eq!(payload["running_capsules"], json!([]));
}

#[tokio::test]
async fn test_system_summary_reports_identity_and_app_id() {
    let dir = tempfile::tempdir().unwrap();

    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["identity"]["handle"], "anders");
    assert!(payload["identity"]["device_did"].is_string());
    assert_eq!(payload["home"]["id"], "home");
    assert_eq!(payload["home"]["route"], "/apps/home/");
    assert_eq!(payload["app"]["id"], "system");
    assert_eq!(payload["app"]["route"], "/apps/system/");
    assert_eq!(payload["runtime"]["running"], false);
    assert_eq!(payload["runtime"]["version"], env!("ELASTOS_VERSION"));
    assert_eq!(payload["source"]["configured"], false);
    assert_eq!(payload["source"]["channel"], "not configured");
    assert_eq!(payload["source"]["installed_version"], "unknown");
    assert_eq!(
        payload["source"]["runtime_version"],
        env!("ELASTOS_VERSION")
    );
    assert_eq!(payload["source"]["update_checks_allowed"], false);
    assert!(payload["source"]["update_policy"]
        .as_str()
        .unwrap()
        .contains("No trusted source configured"));
    assert_eq!(payload["storage"]["available"], false);
    assert_eq!(payload["storage"]["note"], "Document provider unavailable.");
    let webspace_entries = payload["webspace"]["entries"].as_array().unwrap();
    assert!(webspace_entries.iter().any(|entry| {
        entry["id"] == "system"
            && entry["role"] == "app"
            && entry["uri"] == "elastos://capsules/system"
            && entry["route"] == "/apps/system/"
    }));
    assert!(webspace_entries.iter().any(|entry| {
        entry["id"] == "wallet-provider"
            && entry["role"] == "provider"
            && entry["uri"] == "elastos://wallet/*"
            && entry["backend"] == "Wallet authority provider"
    }));
    assert!(payload.get("instance").is_none());
    assert_eq!(payload["runtime_log"]["available"], false);
}

#[tokio::test]
async fn test_system_summary_reports_trusted_source_update_policy() {
    let dir = tempfile::tempdir().unwrap();
    save_trusted_sources(
        dir.path(),
        &TrustedSourcesConfig {
            schema: "elastos.trusted-sources/v1".to_string(),
            default_source: "seed-node-linux".to_string(),
            sources: vec![TrustedSource {
                name: "seed-node-linux".to_string(),
                publisher_dids: vec!["did:key:seedpublisher".to_string()],
                channel: "canary".to_string(),
                discovery_uri: "elastos://source/did:key:seedpublisher/canary".to_string(),
                connect_ticket: "secret-ticket-must-not-render".to_string(),
                gateways: vec!["https://seed.example".to_string()],
                install_path: "/opt/elastos/bin/elastos".to_string(),
                installed_version: "0.5.0-dev".to_string(),
                head_cid: "bafyseedhead".to_string(),
                publisher_node_id: "seed-node-peer-id".to_string(),
                ipns_name: "k51seed".to_string(),
            }],
        },
    )
    .unwrap();

    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["source"]["configured"], true);
    assert_eq!(payload["source"]["name"], "seed-node-linux");
    assert_eq!(payload["source"]["channel"], "canary");
    assert_eq!(payload["source"]["installed_version"], "0.5.0-dev");
    assert_eq!(
        payload["source"]["runtime_version"],
        env!("ELASTOS_VERSION")
    );
    assert_eq!(payload["source"]["source_peer"], "seed-node-peer-id");
    assert!(payload["source"]["transport"]
        .as_str()
        .unwrap()
        .contains("Carrier-first trusted source"));
    assert!(!serde_json::to_string(&payload["source"])
        .unwrap()
        .contains("secret-ticket-must-not-render"));
    if env!("ELASTOS_VERSION").contains("dev") {
        assert_eq!(payload["source"]["mode"], "development");
        assert_eq!(payload["source"]["update_checks_allowed"], false);
    } else {
        assert_eq!(payload["source"]["mode"], "review");
        assert_eq!(payload["source"]["update_checks_allowed"], true);
    }
}

#[tokio::test]
async fn test_system_guest_registration_requires_admin_passkey() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let local_system_token = system_app_token(dir.path());
    let authority = passkey_authority(dir.path());
    let guest = passkey_authority_with_name_role(
        dir.path(),
        Some("guest"),
        crate::auth::RuntimePrincipalRole::Guest,
    );

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/access/guest-registration")
                .header("x-elastos-home-token", local_system_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let guest_denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/access/guest-registration")
                .header("x-elastos-home-token", guest.system_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_denied.status(), StatusCode::FORBIDDEN);

    let enabled = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/access/guest-registration")
                .header("x-elastos-home-token", authority.system_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(enabled.status(), StatusCode::OK);
    let body = axum::body::to_bytes(enabled.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["role"], "admin");
    assert_eq!(payload["guest_registration_enabled"], true);

    let summary = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", authority.system_token)
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
    assert_eq!(payload["access"]["role"], "admin");
    assert_eq!(payload["access"]["guest_registration_enabled"], true);
    assert!(payload["access"]["localhost_root"]
        .as_str()
        .unwrap()
        .starts_with("localhost://Users/"));
}

#[tokio::test]
async fn test_system_summary_reports_storage_counts_when_documents_available() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(documents_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), DOCUMENTS_CAPSULE_ID).unwrap();

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/documents/create")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"System Storage Test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", system_app_token(dir.path()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["storage"]["available"], true);
    assert_eq!(payload["storage"]["documents_count"], 1);
    assert_eq!(payload["storage"]["drafts_count"], 1);
    assert_eq!(payload["storage"]["published_count"], 0);
    assert_eq!(
        payload["storage"]["objects_root"],
        "localhost://ElastOS/Documents/"
    );
}

#[test]
fn system_runtime_activity_filters_attach_noise() {
    use elastos_runtime::primitives::audit::AuditEvent;

    let events = vec![
        AuditEvent::RuntimeStart {
            timestamp: elastos_common::SecureTimestamp::at(10),
            version: "0.1.2-dev".to_string(),
        },
        AuditEvent::SessionCreated {
            timestamp: elastos_common::SecureTimestamp::at(11),
            session_id: "s1".to_string(),
            session_type: "shell".to_string(),
            vm_id: None,
        },
        AuditEvent::PolicyProposal {
            timestamp: elastos_common::SecureTimestamp::at(12),
            request_id: "req-1".to_string(),
            recommended_outcome: "grant".to_string(),
            confidence: 0.9,
            rationale: "noise".to_string(),
        },
        AuditEvent::SecurityWarning {
            timestamp: elastos_common::SecureTimestamp::at(13),
            warning_type: "provider_offline".to_string(),
            details: "localhost-provider missing".to_string(),
        },
        AuditEvent::CapabilityDenied {
            timestamp: elastos_common::SecureTimestamp::at(14),
            request_id: "req-2".to_string(),
            session_id: "s2".to_string(),
            reason: "denied by shell".to_string(),
        },
    ];

    let summaries = system_runtime_activity_summaries(events);
    let rendered = summaries
        .iter()
        .map(|event| event.summary.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        vec![
            "Capability denied — denied by shell",
            "Security warning — provider_offline: localhost-provider missing",
            "Runtime started (0.1.2-dev)",
        ]
    );
}

#[tokio::test]
async fn test_system_handle_update_requires_shell_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let denied = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/identity/handle")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"handle":"anders"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_system_handle_update_rejects_proofless_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let update = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/identity/handle")
                .header("x-elastos-home-token", system_app_token(dir.path()))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"handle":"anders"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(update.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("proof-bound passkey session required"));
    assert!(elastos_identity::load_nickname(dir.path())
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_system_handle_derives_from_passkey_principal() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "principal-handle-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("Anders"));

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", authority.system_token.as_str())
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
    assert_eq!(payload["identity"]["handle"], "Anders");

    let update = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/identity/handle")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"handle":"Anders Admin"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::OK);
    let body = axum::body::to_bytes(update.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["handle"], "Anders Admin");
    assert_eq!(payload["profile_card"]["schema"], "elastos.profile-card/v1");
    assert_eq!(payload["profile_card"]["display_name"], "Anders Admin");
    assert!(payload["profile_card"]["profile_id"]
        .as_str()
        .unwrap()
        .starts_with("profile:local:"));
    assert!(elastos_identity::load_nickname(dir.path())
        .unwrap()
        .is_none());

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", authority.system_token.as_str())
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
    assert_eq!(payload["identity"]["handle"], "Anders Admin");
    assert_eq!(
        payload["identity"]["profile_card"]["display_name"],
        "Anders Admin"
    );

    let chat_launch = app
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
    assert_eq!(chat_launch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(chat_launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let chat_token = payload["route"]
        .as_str()
        .unwrap()
        .split("home_token=")
        .nth(1)
        .unwrap();

    let chat_session = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
                .header("x-elastos-home-token", chat_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(chat_session.status(), StatusCode::OK);
    let body = axum::body::to_bytes(chat_session.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["display_name"], "Anders Admin");
}

#[tokio::test]
async fn test_people_profile_card_update_uses_home_session_token() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "people-profile-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("Anders"));

    let update = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/profile-card")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"handle":"People Name"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::OK);
    let body = axum::body::to_bytes(update.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["handle"], "People Name");
    assert_eq!(payload["profile_card"]["display_name"], "People Name");

    let summary = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.as_str())
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
    assert_eq!(
        payload["identity"]["profile_card"]["display_name"],
        "People Name"
    );
}

#[tokio::test]
async fn test_home_launch_validates_shell_targets() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "launch-peer").await;
    let app = gateway_router(test_state(dir.path()));

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"chat-room"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let ok = app
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
    assert_eq!(ok.status(), StatusCode::OK);
    let body = axum::body::to_bytes(ok.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["target"], "chat-room");
    assert_eq!(payload["target_kind"], "app");
    assert_eq!(payload["launch_status"], "launched");
    assert_eq!(payload["capsule_id"], "wasm-chat-room-instance");
    assert!(payload["route"]
        .as_str()
        .unwrap_or_default()
        .starts_with("/apps/chat-room/?home_token="));

    let library = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"library"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(library.status(), StatusCode::OK);
    let body = axum::body::to_bytes(library.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["target"], "library");
    assert_eq!(payload["title"], "Library");
    assert_eq!(payload["target_kind"], "app");
    assert!(payload["launch_status"].is_null());
    assert!(payload["capsule_id"].is_null());
    assert!(payload["route"]
        .as_str()
        .unwrap_or_default()
        .starts_with("/apps/library/?home_token="));

    let hidden_connector = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"wallet-metamask"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden_connector.status(), StatusCode::OK);
    let body = axum::body::to_bytes(hidden_connector.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["target"], "wallet-metamask");
    assert_eq!(payload["title"], "MetaMask");
    assert!(payload["route"]
        .as_str()
        .unwrap_or_default()
        .starts_with("/apps/wallet-metamask/?home_token="));

    let with_query = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/home/launch")
                    .header("x-elastos-home-token", home_app_token(dir.path()))
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"target":"documents","query":{"doc":"did:key:z6ExampleDoc","view":"read"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    assert_eq!(with_query.status(), StatusCode::OK);
    let body = axum::body::to_bytes(with_query.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap_or_default();
    assert!(route.starts_with("/apps/documents/?home_token="), "{route}");
    assert!(route.contains("doc=did%3Akey%3Az6ExampleDoc"), "{route}");
    assert!(route.contains("view=read"), "{route}");

    let with_elastos_uri = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/home/launch")
                    .header("x-elastos-home-token", home_app_token(dir.path()))
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"target":"documents","query":{"cid":"bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi","uri":"elastos://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi","view":"read"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    assert_eq!(with_elastos_uri.status(), StatusCode::OK);
    let body = axum::body::to_bytes(with_elastos_uri.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap_or_default();
    assert!(route.starts_with("/apps/documents/?home_token="), "{route}");
    assert!(
        route.contains("cid=bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
        "{route}"
    );
    assert!(
        route.contains(
            "uri=elastos%3A%2F%2Fbafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"
        ),
        "{route}"
    );
    assert!(route.contains("view=read"), "{route}");

    let with_peer_invite = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"target":"chat-room","query":{"invite":"elastos://peer/invite?token=abc-123"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(with_peer_invite.status(), StatusCode::OK);
    let body = axum::body::to_bytes(with_peer_invite.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap_or_default();
    assert!(route.starts_with("/apps/chat-room/?home_token="), "{route}");
    assert!(
        route.contains("invite=elastos%3A%2F%2Fpeer%2Finvite%3Ftoken%3Dabc-123"),
        "{route}"
    );

    let viewer = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"gba-ucity"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(viewer.status(), StatusCode::OK);
    let body = axum::body::to_bytes(viewer.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["target"], "gba-ucity");
    assert_eq!(payload["target_kind"], "object");
    assert!(payload["route"]
        .as_str()
        .unwrap()
        .starts_with("/apps/gba-emulator/?capsule=gba-ucity&home_token="));

    let missing = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"missing-shell-target"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_home_active_shell_uses_catalog_shell_candidates() {
    let dir = tempfile::tempdir().unwrap();
    write_test_browser_capsule(
        dir.path(),
        "home-cli",
        "shell",
        "Home CLI",
        Some("<!doctype html><title>Home CLI</title>"),
    );
    write_test_browser_capsule(
        dir.path(),
        "regular-app",
        "app",
        "Regular app",
        Some("<!doctype html><title>Regular App</title>"),
    );
    let broken_shell_dir = dir.path().join("capsules").join("broken-shell");
    std::fs::create_dir_all(&broken_shell_dir).unwrap();
    std::fs::write(
        broken_shell_dir.join("capsule.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": "elastos.capsule/v1",
            "name": "broken-shell",
            "version": "0.1.0",
            "description": "No browser entrypoint",
            "author": "elastos",
            "role": "shell",
            "type": "wasm",
            "entrypoint": "broken-shell.wasm"
        }))
        .unwrap(),
    )
    .unwrap();

    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let stale_state_uri = format!("{localhost_root}/.AppData/ElastOS/Home/active-shell.json");
    let stale_state_path =
        elastos_common::localhost::rooted_localhost_fs_path(dir.path(), &stale_state_uri).unwrap();
    write_home_principal_object_json_for_authority(
        dir.path(),
        &authority,
        "active-shell.json",
        json!({
            "schema": "elastos.home.active-shell/v1",
            "principal_id": authority.principal_id.clone(),
            "localhost_root": localhost_root.clone(),
            "active": "obsolete-shell"
        }),
    );

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.as_str())
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
    assert_eq!(
        payload["active_shell"]["schema"],
        "elastos.home.active-shell/v1"
    );
    assert_eq!(payload["active_shell"]["active"], HOME_GUI_SHELL_ID);
    let repaired_state = std::fs::read_to_string(&stale_state_path).unwrap();
    assert!(!repaired_state.contains("obsolete-shell"));
    assert!(repaired_state.contains(r#""active": "home-gui""#));
    let candidates = payload["active_shell"]["candidates"].as_array().unwrap();
    let candidate_names = candidates
        .iter()
        .map(|candidate| candidate["name"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        candidate_names,
        std::collections::BTreeSet::from([HOME_GUI_SHELL_ID, "home-cli"])
    );
    assert!(candidates
        .iter()
        .any(|candidate| candidate["name"] == HOME_GUI_SHELL_ID
            && candidate["role"] == "shell"
            && candidate["launchable"] == true
            && candidate["route"] == HOME_ROUTE));
    assert!(!candidates
        .iter()
        .any(|candidate| candidate["name"] == HOME_CAPSULE_ID));
    assert!(candidates
        .iter()
        .any(|candidate| candidate["name"] == "home-cli"
            && candidate["role"] == "shell"
            && candidate["launchable"] == true));
    assert!(!candidates
        .iter()
        .any(|candidate| candidate["name"] == "regular-app"));
    assert!(!candidates
        .iter()
        .any(|candidate| candidate["name"] == "broken-shell"));
    let visible_targets = payload["targets"].as_array().unwrap();
    assert!(!visible_targets
        .iter()
        .any(|target| target["target"] == "home-cli"));
    assert!(visible_targets
        .iter()
        .any(|target| target["target"] == "regular-app"
            && target["role"] == "app"
            && target["target_kind"] == "app"));

    let app_rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"regular-app"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(app_rejected.status(), StatusCode::BAD_REQUEST);

    let cli_launch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"home-cli"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cli_launch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(cli_launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let home_cli_token = payload["route"]
        .as_str()
        .unwrap()
        .split("home_token=")
        .nth(1)
        .unwrap()
        .to_string();

    let shell_summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", home_cli_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(shell_summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(shell_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["authority"]["signed_in"], true);
    assert!(payload["targets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|target| target["target"] == "regular-app"));

    let catalog = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/capsules/catalog")
                .header("x-elastos-home-token", home_cli_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(catalog.status(), StatusCode::OK);

    let interfaces = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/capsules/interfaces")
                .header("x-elastos-home-token", home_cli_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(interfaces.status(), StatusCode::OK);

    let esp_initialize = app
        .clone()
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
    assert_eq!(esp_initialize.status(), StatusCode::OK);
    let body = axum::body::to_bytes(esp_initialize.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["protocol"], "elastos-shell-protocol");
    assert_eq!(payload["accepted"][0], "elastos.capsules.catalog/v1");

    let regular_launch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"regular-app"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(regular_launch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(regular_launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let regular_token = payload["route"]
        .as_str()
        .unwrap()
        .split("home_token=")
        .nth(1)
        .unwrap();
    let catalog_rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/capsules/catalog")
                .header("x-elastos-home-token", regular_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(catalog_rejected.status(), StatusCode::FORBIDDEN);

    let selected = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", home_cli_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home-cli"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected.status(), StatusCode::OK);
    let body = axum::body::to_bytes(selected.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active"], "home-cli");

    let selected_from_shell = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", home_cli_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected_from_shell.status(), StatusCode::OK);
    let body = axum::body::to_bytes(selected_from_shell.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active"], "home-cli");

    let selected_gui_from_shell = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", home_cli_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home-gui"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected_gui_from_shell.status(), StatusCode::OK);
    let body = axum::body::to_bytes(selected_gui_from_shell.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active"], HOME_GUI_SHELL_ID);
    let saved_state = std::fs::read_to_string(&stale_state_path).unwrap();
    let saved_state: serde_json::Value = serde_json::from_str(&saved_state).unwrap();
    assert_eq!(saved_state["active"], HOME_GUI_SHELL_ID);

    let selected_from_system = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home-gui"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected_from_system.status(), StatusCode::OK);
    let body = axum::body::to_bytes(selected_from_system.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active"], HOME_GUI_SHELL_ID);

    let selected_cli_from_system = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home-cli"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected_cli_from_system.status(), StatusCode::OK);
    let body = axum::body::to_bytes(selected_cli_from_system.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active"], "home-cli");

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.as_str())
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
    assert_eq!(payload["active_shell"]["active"], "home-cli");

    let cookie_summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header(
                    COOKIE,
                    format!("{}={}", HOME_SESSION_COOKIE, authority.home_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cookie_summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(cookie_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["authority"]["signed_in"], true);
    assert_eq!(payload["active_shell"]["active"], "home-cli");

    let cookie_active_shell_write_rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header(
                    COOKIE,
                    format!("{}={}", HOME_SESSION_COOKIE, authority.home_token),
                )
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home-gui"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        cookie_active_shell_write_rejected.status(),
        StatusCode::FORBIDDEN
    );

    let cookie_active_shell = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/active-shell")
                .header(
                    COOKIE,
                    format!("{}={}", HOME_SESSION_COOKIE, authority.home_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cookie_active_shell.status(), StatusCode::OK);
    let body = axum::body::to_bytes(cookie_active_shell.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active"], "home-cli");
}

#[tokio::test]
async fn test_home_active_shell_repairs_saved_home_state_but_rejects_home_updates() {
    let dir = tempfile::tempdir().unwrap();
    write_test_browser_capsule(
        dir.path(),
        HOME_GUI_SHELL_ID,
        "shell",
        "Home GUI",
        Some("<!doctype html><title>Home GUI</title>"),
    );
    write_test_browser_capsule(
        dir.path(),
        "home-cli",
        "shell",
        "Home CLI",
        Some("<!doctype html><title>Home CLI</title>"),
    );

    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let state_uri = format!("{localhost_root}/.AppData/ElastOS/Home/active-shell.json");
    let state_path =
        elastos_common::localhost::rooted_localhost_fs_path(dir.path(), &state_uri).unwrap();
    write_home_principal_object_json_for_authority(
        dir.path(),
        &authority,
        "active-shell.json",
        json!({
            "schema": "elastos.home.active-shell/v1",
            "principal_id": authority.principal_id.clone(),
            "localhost_root": localhost_root.clone(),
            "active": HOME_CAPSULE_ID
        }),
    );

    let migrated = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(migrated.status(), StatusCode::OK);
    let body = axum::body::to_bytes(migrated.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active"], HOME_GUI_SHELL_ID);
    assert!(payload["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |candidate| candidate["name"] == HOME_GUI_SHELL_ID && candidate["route"] == HOME_ROUTE
        ));
    assert!(!payload["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|candidate| candidate["name"] == HOME_CAPSULE_ID));
    let repaired_state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    assert_eq!(repaired_state["active"], HOME_GUI_SHELL_ID);

    let home_write_rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(home_write_rejected.status(), StatusCode::BAD_REQUEST);
    let saved_state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    assert_eq!(saved_state["active"], HOME_GUI_SHELL_ID);

    let invalid_update = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home-old"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_update.status(), StatusCode::BAD_REQUEST);
    let saved_state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    assert_eq!(saved_state["active"], HOME_GUI_SHELL_ID);
}

#[tokio::test]
async fn test_home_browser_state_is_encrypted_for_protected_principal_root() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let app = gateway_router(test_state(dir.path()));

    let updated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "layout": { "desktopIconsVisible": false },
                        "session": { "openWindows": [] },
                        "recent_targets": ["system"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);

    let path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!(
            "{}/.AppData/ElastOS/Home/browser-state.json",
            protection.localhost_root
        ),
    )
    .unwrap();
    let stored = std::fs::read_to_string(&path).unwrap();
    assert!(!stored.contains("desktopIconsVisible"));
    assert!(stored.contains("elastos.principal-root.object/v1"));
    assert!(stored.contains(&protection.localhost_root));

    let loaded = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(loaded.status(), StatusCode::OK);
    let loaded_body = axum::body::to_bytes(loaded.into_body(), usize::MAX)
        .await
        .unwrap();
    let loaded_json: serde_json::Value = serde_json::from_slice(&loaded_body).unwrap();
    assert_eq!(
        loaded_json["layout"]["desktopIconsVisible"],
        serde_json::Value::Bool(false)
    );
}

#[tokio::test]
async fn test_home_browser_state_drops_unknown_targets() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let desktop_object_entry = format!("object:{localhost_root}/Desktop/Test Folder");
    let trash_entry = format!("object:{localhost_root}/.Trash");
    let foreign_object_entry = "object:localhost://Users/foreign/Desktop/Bad".to_string();
    let mut layout = json!({
        "desktop": {
            "system": { "x": 12, "y": 12 },
            "people": { "x": 18, "y": 18 },
            "obsolete-wallet": { "x": 24, "y": 24 }
        },
        "desktopHidden": ["system", "people", "obsolete-wallet"],
        "desktopLabels": {
            "system": "System",
            "people": "People",
            "obsolete-wallet": "Old Wallet"
        },
        "taskbar": ["system", "people", "obsolete-wallet"],
        "desktopIconsVisible": true
    });
    {
        let desktop = layout["desktop"].as_object_mut().unwrap();
        desktop.insert(desktop_object_entry.clone(), json!({ "x": 36, "y": 36 }));
        desktop.insert(trash_entry.clone(), json!({ "x": 48, "y": 48 }));
        desktop.insert(foreign_object_entry.clone(), json!({ "x": 60, "y": 60 }));
    }
    let app = gateway_router(test_state(dir.path()));

    let updated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "layout": layout,
                        "session": {
                            "browser_context_id": "browser:test",
                            "windows": [
                                { "target": "obsolete-wallet", "active": true },
                                { "target": "people", "active": false },
                                { "target": "system", "active": false }
                            ]
                        },
                        "recent_targets": ["obsolete-wallet", "people", "system"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let body = axum::body::to_bytes(updated.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["layout"]["desktop"].get("obsolete-wallet").is_none());
    assert!(json["layout"]["desktop"].get("people").is_some());
    assert!(json["layout"]["desktop"]
        .get(desktop_object_entry.as_str())
        .is_some());
    assert!(json["layout"]["desktop"]
        .get(trash_entry.as_str())
        .is_some());
    assert!(json["layout"]["desktop"]
        .get(foreign_object_entry.as_str())
        .is_none());
    assert!(json["layout"]["desktopLabels"]
        .get("obsolete-wallet")
        .is_none());
    assert!(json["layout"]["desktopLabels"].get("people").is_some());
    assert_eq!(json["layout"]["desktopHidden"], json!(["system", "people"]));
    assert_eq!(json["layout"]["taskbar"], json!(["system", "people"]));
    assert_eq!(json["session"]["windows"].as_array().unwrap().len(), 2);
    assert_eq!(json["session"]["windows"][0]["target"], "people");
    assert_eq!(json["session"]["windows"][1]["target"], "system");
    assert_eq!(json["recent_targets"], json!(["people", "system"]));
}

#[tokio::test]
async fn test_home_browser_state_recovers_from_malformed_saved_state() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let state_path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!("{localhost_root}/.AppData/ElastOS/Home/browser-state.json"),
    )
    .unwrap();
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    std::fs::write(
        &state_path,
        format!(
            "{}{}",
            serde_json::to_string_pretty(&json!({
                "schema": "elastos.home.browser-state/v1",
                "principal_id": authority.principal_id.clone(),
                "localhost_root": localhost_root.clone(),
                "layout": { "desktopIconsVisible": false },
                "session": { "windows": [] },
                "recent_targets": ["system"]
            }))
            .unwrap(),
            "}"
        ),
    )
    .unwrap();

    let app = gateway_router(test_state(dir.path()));
    let loaded = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(loaded.status(), StatusCode::OK);
    let loaded_body = axum::body::to_bytes(loaded.into_body(), usize::MAX)
        .await
        .unwrap();
    let loaded_json: serde_json::Value = serde_json::from_slice(&loaded_body).unwrap();
    assert_eq!(
        loaded_json["principal_id"].as_str().unwrap(),
        authority.principal_id
    );
    assert!(loaded_json["layout"].is_null());
    assert!(loaded_json["recent_targets"].as_array().unwrap().is_empty());

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);

    let updated = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "layout": { "desktopIconsVisible": true },
                        "session": { "windows": [] },
                        "recent_targets": ["system"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(&state_path).unwrap())
        .expect("Home should rewrite malformed browser state as valid JSON");
    assert_eq!(
        stored["layout"]["desktopIconsVisible"],
        serde_json::Value::Bool(true)
    );
}

#[tokio::test]
async fn test_home_browser_state_resets_plaintext_for_protected_principal_root() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let app = gateway_router(test_state(dir.path()));
    let state_path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!(
            "{}/.AppData/ElastOS/Home/browser-state.json",
            protection.localhost_root
        ),
    )
    .unwrap();
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    std::fs::write(
        &state_path,
        serde_json::to_vec_pretty(&json!({
            "schema": "elastos.home.browser-state/v1",
            "principal_id": authority.principal_id.clone(),
            "localhost_root": protection.localhost_root.clone(),
            "layout": { "desktopIconsVisible": false },
            "session": { "openWindows": [] },
            "recent_targets": ["system"]
        }))
        .unwrap(),
    )
    .unwrap();

    let loaded = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(loaded.status(), StatusCode::OK);
    let loaded_body = axum::body::to_bytes(loaded.into_body(), usize::MAX)
        .await
        .unwrap();
    let loaded_json: serde_json::Value = serde_json::from_slice(&loaded_body).unwrap();
    assert_eq!(
        loaded_json["principal_id"].as_str().unwrap(),
        authority.principal_id
    );
    assert!(loaded_json["layout"].is_null());
    assert!(loaded_json["recent_targets"].as_array().unwrap().is_empty());

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);

    let updated = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "layout": { "desktopIconsVisible": true },
                        "session": { "openWindows": [] },
                        "recent_targets": ["system"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let stored = std::fs::read_to_string(&state_path).unwrap();
    assert!(!stored.contains("desktopIconsVisible"));
    assert!(stored.contains("elastos.principal-root.object/v1"));
}

#[tokio::test]
async fn test_home_launch_starts_system_capsule_and_reports_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "system-peer").await;
    let app = gateway_router(test_state(dir.path()));

    let launch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"system"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(launch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["route"]
        .as_str()
        .unwrap()
        .starts_with("/apps/system/?home_token="));
    assert_eq!(payload["target"], "system");
    assert_eq!(payload["target_kind"], "app");
    assert_eq!(payload["launch_status"], "launched");
    assert_eq!(payload["capsule_id"], "wasm-system-instance");
    let system_token = payload["route"]
        .as_str()
        .unwrap()
        .split("home_token=")
        .nth(1)
        .unwrap();

    let summary = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", system_token)
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
    assert_eq!(payload["runtime"]["running"], true);
    assert!(payload["runtime"]["note"].is_null());
    assert_eq!(payload["runtime_log"]["available"], true);
    assert!(payload["runtime_log"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["kind"] == "capsule_launch"));
}

#[tokio::test]
async fn test_home_launch_starts_chat_room_capsule_and_reports_runtime_activity() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let runtime = start_fake_runtime(dir.path(), bus, "chat-room-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority(dir.path());

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
    assert_eq!(launch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["route"]
        .as_str()
        .unwrap()
        .starts_with("/apps/chat-room/?home_token="));
    assert_eq!(payload["target"], "chat-room");
    assert_eq!(payload["target_kind"], "app");
    assert_eq!(payload["launch_status"], "launched");
    assert_eq!(payload["capsule_id"], "wasm-chat-room-instance");
    let launch_requests = runtime.launch_requests.lock().await;
    let launch_request = launch_requests.last().expect("runtime launch request");
    assert!(
        launch_request.get("principal_id").is_none(),
        "Home must not send raw principal_id authority to runtime launches"
    );
    let launch_grant = launch_request["launch_grant"]
        .as_str()
        .expect("runtime launch request includes signed launch_grant");
    let mut headers = HeaderMap::new();
    headers.insert("x-elastos-home-token", launch_grant.parse().unwrap());
    let (_, grant_context) = require_home_launch_token_for_any_app_context(
        dir.path(),
        &headers,
        &[CHAT_ROOM_CAPSULE_ID],
    )
    .expect("runtime launch grant validates for chat-room");
    assert_eq!(grant_context.principal_id, authority.principal_id);

    let summary = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", authority.system_token.as_str())
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
    assert_eq!(
        payload["runtime"]["running"],
        serde_json::Value::Bool(true),
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(payload["runtime_log"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["kind"] == "capsule_launch"
            && event["summary"]
                .as_str()
                .unwrap_or_default()
                .contains("chat-room")));
}

#[tokio::test]
async fn test_home_launch_materializes_source_wasm_capsule_before_runtime_launch() {
    let dir = tempfile::tempdir().unwrap();
    write_test_browser_capsule(
        dir.path(),
        "archive-manager",
        "app",
        "Archive test capsule",
        None,
    );
    let archive_dir = dir.path().join("capsules").join("archive-manager");
    let built_wasm = archive_dir
        .join("target")
        .join("wasm32-wasip1")
        .join("release")
        .join("archive-manager.wasm");
    std::fs::create_dir_all(built_wasm.parent().unwrap()).unwrap();
    std::fs::write(&built_wasm, b"\0asm").unwrap();
    assert!(!archive_dir.join("archive-manager.wasm").exists());

    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let runtime = start_fake_runtime(dir.path(), bus, "archive-peer").await;
    let app = gateway_router(test_state(dir.path()));

    let launch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"archive-manager"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(launch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["launch_status"], "launched");

    let launch_requests = runtime.launch_requests.lock().await;
    let launch_path = launch_requests
        .last()
        .and_then(|request| request["path"].as_str())
        .expect("runtime launch path");
    assert!(launch_path.ends_with("/dev-capsules/archive-manager"));
    let launch_bundle = std::path::Path::new(launch_path);
    assert!(launch_bundle.join("capsule.json").is_file());
    assert!(launch_bundle.join("archive-manager.wasm").is_file());
    assert!(
        !archive_dir.join("archive-manager.wasm").exists(),
        "source tree should not be dirtied with generated wasm"
    );
}

#[tokio::test]
async fn test_home_launch_reports_system_launch_failure_when_runtime_cannot_start() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let launch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"system"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(launch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["route"]
        .as_str()
        .unwrap()
        .starts_with("/apps/system/?home_token="));
    assert_eq!(payload["launch_status"], "failed");
    assert!(payload["launch_detail"]
        .as_str()
        .unwrap()
        .contains("managed local runtime could not start"));
    let system_token = payload["route"]
        .as_str()
        .unwrap()
        .split("home_token=")
        .nth(1)
        .unwrap();

    let summary = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", system_token)
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
    assert_eq!(payload["runtime"]["running"], false);
    assert_eq!(payload["runtime_log"]["available"], false);
    assert!(payload["runtime_log"]["note"]
        .as_str()
        .unwrap()
        .contains("Local runtime is not running"));
}

#[test]
fn resolve_capsule_dir_prefers_installed_capsule_before_dev_tree_copy() {
    let dir = tempfile::tempdir().unwrap();
    write_test_capsule_manifest(dir.path(), SYSTEM_CAPSULE_ID);

    let capsule_dir =
        resolve_capsule_dir(dir.path(), SYSTEM_CAPSULE_ID).expect("installed system capsule path");
    assert_eq!(
        capsule_dir,
        dir.path().join("capsules").join(SYSTEM_CAPSULE_ID)
    );
}

fn assert_rejects_unknown_gateway_field<T: serde::de::DeserializeOwned>(value: serde_json::Value) {
    let err = match serde_json::from_value::<T>(value) {
        Ok(_) => panic!("expected request body to reject unknown fields"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("unknown field"), "{err}");
}

#[test]
fn test_system_request_bodies_reject_hidden_authority_fields() {
    assert_rejects_unknown_gateway_field::<HomeBrowserStateUpdate>(json!({
        "session": null,
        "principal_id": "person:local:other"
    }));
    assert_rejects_unknown_gateway_field::<HomeActiveShellUpdate>(json!({
        "active": "home-gui",
        "route": "/apps/home/"
    }));
    assert_rejects_unknown_gateway_field::<SystemHandleUpdateRequest>(json!({
        "handle": "alice",
        "did": "did:elastos:alice"
    }));
    assert_rejects_unknown_gateway_field::<SystemBackgroundOverlayRequest>(json!({
        "enabled": true,
        "opacity": 0.25,
        "storage_path": "localhost://Users/self"
    }));
    assert_rejects_unknown_gateway_field::<SystemGuestRegistrationRequest>(json!({
        "enabled": true,
        "role": "admin"
    }));
}

#[test]
fn test_wallet_request_bodies_reject_hidden_authority_fields() {
    assert_rejects_unknown_gateway_field::<WalletApprovalRejectRequest>(json!({
        "reason": "no",
        "force": true
    }));
    assert_rejects_unknown_gateway_field::<WalletApprovalApproveRequest>(json!({
        "reason": "ok",
        "raw_signature": "0x00"
    }));
    assert_rejects_unknown_gateway_field::<WalletApprovalCompleteRequest>(json!({
        "payload_hash": "hash",
        "signature": "0xsig",
        "signer": "0xsigner",
        "private_key": "must-not-be-accepted"
    }));
    assert_rejects_unknown_gateway_field::<SystemWalletManagedCreateRequest>(json!({
        "chain_namespace": "eip155:20",
        "label": "Built-in",
        "seed_phrase": "must-not-be-accepted"
    }));
    assert_rejects_unknown_gateway_field::<SystemWalletDefaultRequest>(json!({
        "account_id": "account:test",
        "chain_namespace": "eip155:20",
        "intent": "personal_sign",
        "rpc_url": "https://example.invalid"
    }));
}

#[test]
fn test_home_and_inbox_request_bodies_reject_hidden_authority_fields() {
    assert_rejects_unknown_gateway_field::<HomeLaunchRequest>(json!({
        "target": "chat-room",
        "principal_id": "person:local:other"
    }));
    assert_rejects_unknown_gateway_field::<InboxActionRequest>(json!({
        "action_id": "wallet:test",
        "approve": true
    }));
}

#[test]
fn test_chat_request_bodies_reject_hidden_identity_fields() {
    assert_rejects_unknown_gateway_field::<RoomPollBody>(json!({
        "since": 1,
        "principal_id": "person:local:other"
    }));
    assert_rejects_unknown_gateway_field::<RoomSendBody>(json!({
        "body": "hello",
        "sender_id": "did:key:forged"
    }));
    assert_rejects_unknown_gateway_field::<ChatRoomAccessPolicyBody>(json!({
        "allow_guest_invites": true,
        "allow_member_invites": true,
        "allow_members_to_host_guests": false,
        "admin_override": true
    }));
    assert_rejects_unknown_gateway_field::<ChatRoomMemberInviteBody>(json!({
        "member_did": "did:key:z6Mktest",
        "capability_token": "must-not-be-accepted"
    }));
    assert_rejects_unknown_gateway_field::<ChatRoomMemberRemoveBody>(json!({
        "member_did": "did:key:z6Mktest",
        "delete_history": true
    }));
    assert_rejects_unknown_gateway_field::<ChatRoomInviteRevokeBody>(json!({
        "invite_id": "invite:test",
        "member_did": "did:key:z6Mktest"
    }));
    assert_rejects_unknown_gateway_field::<RoomUploadStartBody>(json!({
        "file_name": "note.md",
        "mime_type": "text/markdown",
        "size_bytes": 10,
        "ipfs_gateway": "https://example.invalid/ipfs"
    }));
}
