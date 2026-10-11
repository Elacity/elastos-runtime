use super::*;

const ADMIN_PASSKEY: &str = "gateway-test-passkey";
const GUEST_PASSKEY: &str = "gateway-test-guest-passkey";

fn admin_sign_in(dir: &std::path::Path) -> TestPasskeyAuthority {
    passkey_authority_with_name_role_credential(
        dir,
        Some("admin"),
        crate::auth::RuntimePrincipalRole::Admin,
        ADMIN_PASSKEY,
    )
}

fn guest_sign_in(dir: &std::path::Path) -> TestPasskeyAuthority {
    passkey_authority_with_name_role_credential(
        dir,
        Some("guest"),
        crate::auth::RuntimePrincipalRole::Guest,
        GUEST_PASSKEY,
    )
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    // Home tokens come from the Home page; System tokens from its sandboxed frame.
    let origin = if uri == "/api/apps/home/summary" || uri == "/api/auth/sessions/refresh" {
        "http://localhost:61180"
    } else {
        "null"
    };
    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", origin)
                .method(method)
                .uri(uri)
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
    let payload = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, payload)
}

async fn listed_session_ids(app: &axum::Router, token: &str) -> Vec<String> {
    let (status, payload) = send(app, "GET", "/api/auth/sessions", token).await;
    assert_eq!(status, StatusCode::OK, "{payload}");
    payload["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["session_id"].as_str().unwrap().to_string())
        .collect()
}

async fn home_summary_status(app: &axum::Router, token: &str) -> StatusCode {
    send(app, "GET", "/api/apps/home/summary", token).await.0
}

#[tokio::test]
async fn session_list_shows_only_the_callers_own_sign_ins() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let laptop = admin_sign_in(dir.path());
    let phone = admin_sign_in(dir.path());
    let guest = guest_sign_in(dir.path());
    crate::auth::set_session_label(dir.path(), &phone.session_id, "Safari on iPhone").unwrap();

    let (status, listed) = send(&app, "GET", "/api/auth/sessions", &laptop.system_token).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["schema"], "elastos.auth.sessions/v1");
    assert_eq!(
        listed["max_age_secs"].as_u64(),
        Some(crate::auth::AUTH_SESSION_MAX_AGE_SECS)
    );
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2, "an admin sees only their own sign-ins");
    let current = sessions
        .iter()
        .find(|session| session["session_id"] == laptop.session_id.as_str())
        .unwrap();
    assert_eq!(current["current"], true);
    assert!(current["label"].is_null());
    let other = sessions
        .iter()
        .find(|session| session["session_id"] == phone.session_id.as_str())
        .unwrap();
    assert_eq!(other["current"], false);
    assert_eq!(other["label"], "Safari on iPhone");
    assert_eq!(
        other["sign_in_again_by"].as_u64().unwrap(),
        other["created_at"].as_u64().unwrap() + crate::auth::AUTH_SESSION_MAX_AGE_SECS
    );

    assert_eq!(
        listed_session_ids(&app, &guest.system_token).await,
        vec![guest.session_id.clone()]
    );
}

#[tokio::test]
async fn guest_revokes_own_other_sign_in_but_not_another_users() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let admin = admin_sign_in(dir.path());
    let guest_laptop = guest_sign_in(dir.path());
    let guest_phone = guest_sign_in(dir.path());

    let (status, _) = send(
        &app,
        "POST",
        &format!("/api/auth/sessions/{}/revoke", admin.session_id),
        &guest_laptop.system_token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        listed_session_ids(&app, &admin.system_token).await,
        vec![admin.session_id.clone()]
    );

    let (status, revoked) = send(
        &app,
        "POST",
        &format!("/api/auth/sessions/{}/revoke", guest_phone.session_id),
        &guest_laptop.system_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(
        home_summary_status(&app, &guest_phone.home_token).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "GET", "/api/auth/sessions", &guest_phone.system_token)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        listed_session_ids(&app, &guest_laptop.system_token).await,
        vec![guest_laptop.session_id.clone()]
    );
}

#[tokio::test]
async fn sign_out_everywhere_else_keeps_this_sign_in_and_other_users() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let laptop = admin_sign_in(dir.path());
    let phone = admin_sign_in(dir.path());
    let tablet = admin_sign_in(dir.path());
    let guest = guest_sign_in(dir.path());

    let (status, payload) = send(
        &app,
        "POST",
        "/api/auth/sessions/revoke-others",
        &laptop.system_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{payload}");
    assert_eq!(payload["revoked"], 2);
    for signed_out in [&phone, &tablet] {
        assert_eq!(
            home_summary_status(&app, &signed_out.home_token).await,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        listed_session_ids(&app, &laptop.system_token).await,
        vec![laptop.session_id.clone()]
    );
    assert_eq!(
        listed_session_ids(&app, &guest.system_token).await,
        vec![guest.session_id.clone()]
    );
}

#[tokio::test]
async fn revoked_sign_in_stays_refused_after_runtime_restart() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let laptop = admin_sign_in(dir.path());
    let phone = admin_sign_in(dir.path());
    let (status, _) = send(
        &app,
        "POST",
        &format!("/api/auth/sessions/{}/revoke", phone.session_id),
        &laptop.system_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    drop(app);

    let restarted = gateway_router(test_state(dir.path()));
    assert_eq!(
        send(&restarted, "GET", "/api/auth/sessions", &phone.system_token)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        home_summary_status(&restarted, &phone.home_token).await,
        StatusCode::FORBIDDEN
    );
    let (status, _) = send(
        &restarted,
        "POST",
        "/api/auth/sessions/refresh",
        &phone.home_token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        listed_session_ids(&restarted, &laptop.system_token).await,
        vec![laptop.session_id.clone()]
    );
}
