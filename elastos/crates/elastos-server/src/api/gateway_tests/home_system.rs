use super::*;

#[tokio::test]
async fn test_home_static_route_serves_browser_surface() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

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
    assert!(text.contains("./shell.js"));

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
        .oneshot(
            Request::builder()
                .uri("/apps/home/shell.js")
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
}

#[tokio::test]
async fn test_home_summary_reports_identity_and_launch_targets() {
    let dir = tempfile::tempdir().unwrap();

    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
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
    assert_eq!(library["target_kind"], "object");
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
    assert!(elastos_identity::load_nickname(dir.path())
        .unwrap()
        .is_none());

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
    assert_eq!(payload["target_kind"], "object");
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
                        "layout": {
                            "desktop": {
                                "system": { "x": 12, "y": 12 },
                                "obsolete-wallet": { "x": 24, "y": 24 }
                            },
                            "desktopHidden": ["system", "obsolete-wallet"],
                            "desktopLabels": {
                                "system": "System",
                                "obsolete-wallet": "Old Wallet"
                            },
                            "taskbar": ["system", "obsolete-wallet"],
                            "desktopIconsVisible": true
                        },
                        "session": {
                            "browser_context_id": "browser:test",
                            "windows": [
                                { "target": "obsolete-wallet", "active": true },
                                { "target": "system", "active": false }
                            ]
                        },
                        "recent_targets": ["obsolete-wallet", "system"]
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
    assert!(json["layout"]["desktopLabels"]
        .get("obsolete-wallet")
        .is_none());
    assert_eq!(json["layout"]["desktopHidden"], json!(["system"]));
    assert_eq!(json["layout"]["taskbar"], json!(["system"]));
    assert_eq!(json["session"]["windows"].as_array().unwrap().len(), 1);
    assert_eq!(json["session"]["windows"][0]["target"], "system");
    assert_eq!(json["recent_targets"], json!(["system"]));
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
