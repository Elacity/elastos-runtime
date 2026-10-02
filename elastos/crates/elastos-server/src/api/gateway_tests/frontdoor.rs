use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn app(root: &std::path::Path) -> Router {
    gateway_router_with_api_url(test_state(root), "http://localhost:61180".to_string())
}

#[tokio::test]
async fn frontdoor_refuses_foreign_origins_rebinding_and_missing_or_duplicate_host() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    for (host, origin) in [
        (Some("localhost:61180"), Some("http://localhost:61181")),
        (Some("127.0.0.1:61180"), Some("http://127.0.0.1:9999")),
        (Some("rebind.example:61180"), None),
        (Some("localhost.attacker.test:61180"), None),
        (Some("localhost:61181"), None),
        (None, None),
    ] {
        let mut request = Request::builder().uri("/healthz");
        if let Some(host) = host {
            request = request.header(HOST, host);
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{host:?} {origin:?}"
        );
    }
    let duplicate = Request::builder()
        .uri("/healthz")
        .header(HOST, "localhost:61180")
        .header(HOST, "rebind.example")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(duplicate).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let valid = Request::builder()
        .uri("/healthz")
        .header(HOST, "127.0.0.1:61180")
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.oneshot(valid).await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn frontdoor_refuses_opaque_cookie_and_forged_token_before_reading_100mb() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("frontdoor"));
    let app = app(dir.path());
    let assistant_token = app_token_for_authority(dir.path(), "assistant", &authority);
    for credential in [
        None,
        Some("invalid"),
        Some(authority.home_token.as_str()),
        Some(assistant_token.as_str()),
    ] {
        let read = Arc::new(AtomicUsize::new(0));
        let counter = read.clone();
        let stream = futures_lite::stream::unfold(0, move |index| {
            let counter = counter.clone();
            async move {
                if index == 1600 {
                    return None;
                }
                counter.fetch_add(64 * 1024, Ordering::SeqCst);
                Some((
                    Ok::<_, Infallible>(Bytes::from(vec![b'x'; 64 * 1024])),
                    index + 1,
                ))
            }
        });
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/apps/home/state")
            .header(HOST, "localhost:61180")
            .header("origin", "null")
            .header(CONTENT_TYPE, "application/json")
            .header("content-length", 100 * 1024 * 1024);
        if let Some(token) = credential {
            request = request.header("x-elastos-home-token", token);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from_stream(stream)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            read.load(Ordering::SeqCst),
            0,
            "the authority gate precedes every body poll"
        );
    }
}

#[tokio::test]
async fn frontdoor_only_admits_signed_launches_to_private_opaque_apis() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("frontdoor"));
    let app = app(dir.path());
    for path in [
        "/api/apps/home/summary",
        "/api/carrier/bootstrap",
        "/.well-known/elastos/carrier-bootstrap.json?role=publisher",
        "/api/capsules/catalog",
        "/api/auth/passkey/status",
    ] {
        let request = Request::builder()
            .uri(path)
            .header(HOST, "localhost:61180")
            .header("origin", "null")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    let assistant_token = app_token_for_authority(dir.path(), "assistant", &authority);
    let request = test_browser_request("localhost:61180", "null")
        .uri("/api/apps/assistant/workspace")
        .header("x-elastos-home-token", &assistant_token)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::OK
    );
    let request = test_browser_request("localhost:61180", "http://localhost:61180")
        .uri("/api/apps/home/summary")
        .header("x-elastos-home-token", &authority.home_token)
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.oneshot(request).await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn frontdoor_public_list_requires_configured_gateway_host() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "gateway_allowed_hosts = [\"home.example.test\"]\n",
    )
    .unwrap();
    let app = app(dir.path());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .header(HOST, "home.example.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .header(HOST, "other.example.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn frontdoor_runtime_operator_sessions_keep_exact_paths_and_prebody_refusal() {
    use elastos_runtime::{
        primitives::audit::AuditLog,
        session::{SessionRegistry, SessionType},
    };
    let dir = tempfile::tempdir().unwrap();
    let sessions = Arc::new(SessionRegistry::new(Arc::new(AuditLog::new())));
    let _service = register_browser_operator_sessions(dir.path(), sessions.clone());
    let session = sessions.create_session(SessionType::Capsule, None).await;
    let expired = sessions.create_session(SessionType::Capsule, None).await;
    sessions.invalidate_session(&expired.token).await.unwrap();
    let app = app(dir.path());
    for (path, token, origin, expected) in [
        (
            "/api/apps/browser/pages/page/operator-requests",
            "forged",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            "/api/apps/browser/pages/page/operator-requests",
            expired.token.as_str(),
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            "/api/apps/browser/pages/page/operator-requests",
            session.token.as_str(),
            Some("null"),
            StatusCode::FORBIDDEN,
        ),
        (
            "/api/apps/browser/pages/page/operator-requests",
            session.token.as_str(),
            Some("http://localhost:61181"),
            StatusCode::FORBIDDEN,
        ),
        (
            "/api/apps/home/state",
            session.token.as_str(),
            None,
            StatusCode::FORBIDDEN,
        ),
        (
            "/api/apps/browser/pages/page/operator-requests/id",
            session.token.as_str(),
            None,
            StatusCode::FORBIDDEN,
        ),
    ] {
        let read = Arc::new(AtomicUsize::new(0));
        let counter = read.clone();
        let stream = futures_lite::stream::unfold(0, move |index| {
            let counter = counter.clone();
            async move {
                if index == 1600 {
                    return None;
                }
                counter.fetch_add(64 * 1024, Ordering::SeqCst);
                Some((
                    Ok::<_, Infallible>(Bytes::from(vec![b'x'; 64 * 1024])),
                    index + 1,
                ))
            }
        });
        let mut request = Request::builder()
            .method("POST")
            .uri(path)
            .header(HOST, "localhost:61180")
            .header("authorization", format!("Bearer {token}"))
            .header(CONTENT_TYPE, "application/json")
            .header("content-length", 100 * 1024 * 1024);
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from_stream(stream)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{path} {origin:?}");
        assert_eq!(
            read.load(Ordering::SeqCst),
            0,
            "unadmitted session reads no body"
        );
    }
}
