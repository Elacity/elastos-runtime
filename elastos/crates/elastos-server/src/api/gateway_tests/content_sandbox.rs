use super::*;

const CONTENT_POLICY: &str = "sandbox allow-scripts allow-forms allow-popups; connect-src 'none'";
const PAGE: &str = r#"<!doctype html><title>Content sandbox fixture</title>
<link rel="stylesheet" href="style.css"><script src="probe.js" defer></script>
<img id="image" src="image.svg"><form id="form"><input name="message" required>
<button>Submit</button><output id="result"></output></form>
<a id="download" href="download.bin" download>Download</a>"#;
const STYLE: &str = "#result { color: rgb(12, 34, 56); }";
const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="green"/></svg>"#;
const DOWNLOAD: &[u8] = b"\x00sandbox-download\xff";
const PROBE: &str = r#"
window.probe = { script: true, violations: [], fetches: [] };
document.addEventListener('securitypolicyviolation', event => {
  probe.violations.push(event.effectiveDirective);
});
for (const [name, read] of Object.entries({
  cookie: () => document.cookie,
  localStorage: () => localStorage.getItem('runtime-sandbox-sentinel'),
  sessionStorage: () => sessionStorage.getItem('runtime-sandbox-sentinel'),
  parentDocument: () => parent.document.title,
  parentStorage: () => parent.localStorage.getItem('runtime-sandbox-sentinel')
})) {
  try { probe[name] = { value: read(), denied: false }; }
  catch (error) { probe[name] = { denied: error.name === 'SecurityError' }; }
}
document.querySelector('#form').addEventListener('submit', event => {
  event.preventDefault();
  document.querySelector('#result').textContent = new FormData(event.target).get('message');
});
(async () => {
  for (const [path, method, body] of [
    ['/api/apps/home/summary', 'GET'],
    ['/api/apps/home/state', 'GET'],
    ['/api/apps/home/state', 'POST', '{}'],
    ['/api/apps/assistant/workspace', 'GET'],
    ['/api/apps/assistant/workspace', 'PUT', '{"schema":"elastos.assistant.workspace/v1","if_revision":0,"draft":"attack"}']
  ]) {
    try {
      const response = await fetch(path, { method, body, credentials: 'include',
        headers: body ? { 'content-type': 'application/json' } : undefined });
      probe.fetches.push({ path, method, blocked: false, status: response.status });
    } catch (error) { probe.fetches.push({ path, method, blocked: error.name === 'TypeError' }); }
  }
  probe.ready = true;
})();
"#;

fn write_content_fixture(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("directory")).unwrap();
    for (path, bytes) in [
        ("index.html", PAGE.as_bytes()),
        ("directory/index.html", PAGE.as_bytes()),
        ("probe.js", PROBE.as_bytes()),
        ("style.css", STYLE.as_bytes()),
        ("image.svg", SVG.as_bytes()),
        ("download.bin", DOWNLOAD),
    ] {
        std::fs::write(root.join(path), bytes).unwrap();
    }
}

fn assert_content_policy(response: &Response, path: &str) {
    assert_eq!(
        response
            .headers()
            .get("content-security-policy")
            .and_then(|v| v.to_str().ok()),
        Some(CONTENT_POLICY),
        "content boundary at {path}"
    );
}

#[tokio::test]
async fn content_sandbox_cid_aliases_preserve_bytes_and_types() {
    let dir = tempfile::tempdir().unwrap();
    write_content_fixture(&dir.path().join(TEST_CIDV1));
    std::fs::write(dir.path().join(format!("{TEST_CIDV0}.raw")), DOWNLOAD).unwrap();
    let app = gateway_router(test_state(dir.path()));
    for prefix in [format!("/s/{TEST_CIDV1}"), format!("/ipfs/{TEST_CIDV1}")] {
        for (suffix, bytes, mime) in [
            ("/", PAGE.as_bytes(), "text/html; charset=utf-8"),
            ("/index.html", PAGE.as_bytes(), "text/html; charset=utf-8"),
            (
                "/directory/index.html",
                PAGE.as_bytes(),
                "text/html; charset=utf-8",
            ),
            ("/probe.js", PROBE.as_bytes(), "application/javascript"),
            ("/style.css", STYLE.as_bytes(), "text/css"),
            ("/image.svg", SVG.as_bytes(), "image/svg+xml"),
            ("/download.bin", DOWNLOAD, "application/octet-stream"),
        ] {
            let path = format!("{prefix}{suffix}");
            let response = app
                .clone()
                .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_content_policy(&response, &path);
            assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), mime);
            assert_eq!(
                &axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap()[..],
                bytes,
                "{path}"
            );
        }
    }
    for path in [
        format!("/content/{TEST_CIDV1}"),
        format!("/ipfs/{TEST_CIDV1}"),
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_content_policy(&response, &path);
        assert_eq!(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()[..],
            PAGE.as_bytes()
        );
    }
    for path in [
        format!("/content/{TEST_CIDV0}"),
        format!("/ipfs/{TEST_CIDV0}"),
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_content_policy(&response, &path);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
        assert_eq!(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()[..],
            DOWNLOAD
        );
    }
    for (path, status) in [
        (format!("/s/{TEST_CIDV1}"), StatusCode::PERMANENT_REDIRECT),
        ("/s/invalid/".to_string(), StatusCode::BAD_REQUEST),
        ("/ipfs/invalid".to_string(), StatusCode::BAD_REQUEST),
        ("/content/invalid".to_string(), StatusCode::BAD_REQUEST),
        (
            format!("/s/{TEST_CIDV1}/missing.html"),
            StatusCode::NOT_FOUND,
        ),
        (format!("/s/{TEST_CIDV1}/directory/"), StatusCode::NOT_FOUND),
        (
            format!("/ipfs/{TEST_CIDV1}/../escape"),
            StatusCode::BAD_REQUEST,
        ),
        (format!("/s/{TEST_CIDV0}/"), StatusCode::NOT_FOUND),
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{path}");
        assert_content_policy(&response, &path);
    }
    for (path, mime, cached_path) in [
        (
            format!("/s/{TEST_CIDV1}/index.html"),
            "text/html; charset=utf-8",
            std::path::PathBuf::from(TEST_CIDV1).join("index.html"),
        ),
        (
            format!("/content/{TEST_CIDV1}"),
            "application/octet-stream",
            std::path::PathBuf::from(format!("{TEST_CIDV1}.raw")),
        ),
    ] {
        let fetched_dir = tempfile::tempdir().unwrap();
        let fetched = gateway_router(content_test_state(fetched_dir.path()).await)
            .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(fetched.status(), StatusCode::OK);
        assert_content_policy(&fetched, &path);
        assert_eq!(fetched.headers().get(CONTENT_TYPE).unwrap(), mime);
        let bytes = axum::body::to_bytes(fetched.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(!bytes.is_empty());
        assert_eq!(
            &bytes[..],
            std::fs::read(fetched_dir.path().join(cached_path)).unwrap()
        );
    }
}

#[tokio::test]
async fn content_sandbox_site_files_and_admitted_bundle_preserve_provenance() {
    let dir = tempfile::tempdir().unwrap();
    write_content_fixture(&my_website_root_path(dir.path()));
    write_content_fixture(&dir.path().join(TEST_CIDV1));
    let app = gateway_router(test_state(dir.path()));
    for bundle in [false, true] {
        if bundle {
            let head_path = edge_site_head_path(dir.path(), MY_WEBSITE_URI);
            std::fs::create_dir_all(head_path.parent().unwrap()).unwrap();
            std::fs::write(head_path, serde_json::to_vec(&json!({
                "payload": {"schema":"elastos.site.head.v1", "target": MY_WEBSITE_URI,
                    "bundle_cid": TEST_CIDV1, "release_name":"fixture", "channel_name":"test",
                    "content_digest":"sha256:fixture", "entry_count":6, "total_bytes":100, "activated_at":1},
                "signature":"fixture", "signer_did":"did:key:fixture"
            })).unwrap()).unwrap();
        }
        for (path, bytes, mime) in [
            ("/", PAGE.as_bytes(), "text/html; charset=utf-8"),
            ("/directory", PAGE.as_bytes(), "text/html; charset=utf-8"),
            ("/probe.js", PROBE.as_bytes(), "application/javascript"),
            ("/style.css", STYLE.as_bytes(), "text/css"),
            ("/image.svg", SVG.as_bytes(), "image/svg+xml"),
            ("/download.bin", DOWNLOAD, "application/octet-stream"),
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_content_policy(&response, path);
            assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), mime);
            assert_eq!(
                response.headers().get("x-elastos-site-origin").unwrap(),
                MY_WEBSITE_URI
            );
            if bundle {
                for (header, value) in [
                    ("x-elastos-site-head-schema", "elastos.site.head.v1"),
                    ("x-elastos-site-head-digest", "sha256:fixture"),
                    ("x-elastos-site-head-cid", TEST_CIDV1),
                    ("x-elastos-site-head-signer", "did:key:fixture"),
                    ("x-elastos-site-head-release", "fixture"),
                    ("x-elastos-site-head-channel", "test"),
                ] {
                    assert_eq!(response.headers().get(header).unwrap(), value);
                }
            }
            assert_eq!(
                &axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap()[..],
                bytes
            );
        }
        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/missing.html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_content_policy(&missing, "/missing.html");
    }
    let home = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/home/")
                .header(HOST, "localhost:61180")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(home.status(), StatusCode::OK);
    assert_eq!(home.headers().get("content-security-policy").unwrap(),
        "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' blob: data:; connect-src 'self'; frame-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'");
    let health = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    assert!(health.headers().get("content-security-policy").is_none());
}

#[tokio::test]
async fn content_sandbox_opaque_cookie_requests_require_explicit_launch_authority() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let bootstrap = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        bootstrap.status(),
        StatusCode::FORBIDDEN,
        "Home summary now requires its signed session"
    );
    for role in [
        crate::auth::RuntimePrincipalRole::Admin,
        crate::auth::RuntimePrincipalRole::Guest,
    ] {
        let authority = passkey_authority_with_name_role(dir.path(), Some("sandbox-test"), role);
        let headers = test_browser_request("localhost:61180", "null")
            .body(Body::empty())
            .unwrap()
            .into_parts()
            .0
            .headers;
        let cookie = format!(
            "{}={}",
            home_session_cookie_name(&headers).unwrap(),
            authority.home_token
        );
        let assistant_token = app_token_for_authority(dir.path(), "assistant", &authority);
        let before = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .uri("/api/apps/assistant/workspace")
                    .header("x-elastos-home-token", &assistant_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            before.status(),
            StatusCode::OK,
            "authorized opaque app fetch remains available"
        );
        let before = axum::body::to_bytes(before.into_body(), usize::MAX)
            .await
            .unwrap();
        for (path, method, body) in [
            ("/api/apps/home/state", "GET", ""),
            ("/api/apps/home/state", "POST", "{}"),
            ("/api/apps/assistant/workspace", "GET", ""),
            (
                "/api/apps/assistant/workspace",
                "PUT",
                r#"{"schema":"elastos.assistant.workspace/v1","if_revision":0,"draft":"attack"}"#,
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    test_browser_request("localhost:61180", "null")
                        .method(method)
                        .uri(path)
                        .header(COOKIE, &cookie)
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "cookie-only {method} {path}"
            );
        }
        for metadata in [
            [("origin", "null"), ("sec-fetch-mode", "navigate")],
            [
                ("sec-fetch-site", "cross-site"),
                ("sec-fetch-dest", "document"),
            ],
            [
                ("sec-fetch-site", "cross-site"),
                ("sec-fetch-dest", "iframe"),
            ],
        ] {
            let mut request = Request::builder()
                .uri("/api/apps/home/summary")
                .header(HOST, "localhost:61180")
                .header(COOKIE, &cookie);
            for (name, value) in metadata {
                request = request.header(name, value);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "opaque content navigation keeps Home authority private"
            );
        }
        let after = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .uri("/api/apps/assistant/workspace")
                    .header("x-elastos-home-token", &assistant_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(after.status(), StatusCode::OK);
        assert_eq!(
            axum::body::to_bytes(after.into_body(), usize::MAX)
                .await
                .unwrap(),
            before,
            "refused content requests leave the app workspace unchanged"
        );
    }
}

fn copy_operator_site(source: &std::path::Path, destination: &std::path::Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_operator_site(&entry.path(), &destination.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
        }
    }
}

fn write_frontdoor_browser_fixture(data_dir: &std::path::Path, repo: &std::path::Path) {
    // The real host boot must recover from anonymous API admission failures.
    copy_operator_site(
        &repo.join("capsules/home/browser"),
        &data_dir.join("capsules/home/browser"),
    );
    let root = data_dir.join("capsules/assistant/browser");
    let html = "<!doctype html><title>Hostile app fixture</title><script src=\"./probe.js\" defer></script><a id=\"escape\" href=\"/apps/assistant/extra.html\" target=\"_blank\">Open app document</a>";
    for name in ["index.html", "extra.html"] {
        std::fs::write(root.join(name), html).unwrap();
    }
    std::fs::write(root.join("extra.svg"), r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><script href="probe.js"/><rect width="16" height="16" fill="green"/></svg>"#).unwrap();
    std::fs::write(
        root.join("probe.js"),
        r#"
window.appProbe = { script: true, reads: {}, requests: [] };
appProbe.compilation = { wasm: false, javascript: 'allowed' };
try {
  appProbe.compilation.wasm = new WebAssembly.Module(new Uint8Array([0,97,115,109,1,0,0,0])) instanceof WebAssembly.Module;
} catch (error) { appProbe.compilation.wasmError = error.name; }
try { new Function('return 1')(); }
catch (error) { appProbe.compilation.javascript = error.name; }
for (const [name, read] of Object.entries({
  cookie: () => document.cookie,
  storage: () => localStorage.getItem('runtime-sandbox-sentinel')
})) {
  try { appProbe.reads[name] = { value: read(), denied: false }; }
  catch (error) { appProbe.reads[name] = { denied: error.name === 'SecurityError' }; }
}
window.runAppRequests = async token => {
  const results = [];
  for (const path of ['/api/apps/assistant/workspace', '/api/apps/home/summary']) {
    try {
      const response = await fetch(path, { credentials: token ? 'omit' : 'include',
        headers: token ? { 'x-elastos-home-token': token } : {} });
      results.push({ path, status: response.status, body: await response.text() });
    } catch (error) { results.push({ path, failed: error.name }); }
  }
  appProbe.requests = results;
  return results;
};
appProbe.ready = true;
"#,
    )
    .unwrap();
}

#[tokio::test]
#[ignore = "requires Playwright and Chromium; CI runs this browser boundary regression"]
async fn content_sandbox_browser() {
    let dir = tempfile::tempdir().unwrap();
    write_content_fixture(&dir.path().join(TEST_CIDV1));
    let site_root = my_website_root_path(dir.path());
    write_content_fixture(&site_root);
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    copy_operator_site(&repo.join("website/elastos"), &site_root.join("operator"));
    let authority = passkey_authority_with_name(dir.path(), Some("browser-sandbox-fixture"));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = test_state(dir.path());
    write_frontdoor_browser_fixture(dir.path(), &repo);
    let assistant_token = app_token_for_authority(dir.path(), "assistant", &authority);
    let app = gateway_router_with_api_url(state, format!("http://{addr}"));
    let mut headers = HeaderMap::new();
    headers.insert(HOST, addr.to_string().parse().unwrap());
    let cookie_name = home_session_cookie_name(&headers).unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let mut server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        })
        .await
    });
    let mut command = tokio::process::Command::new("node");
    command
        .arg(repo.join("scripts/gateway-content-sandbox-browser-smoke.mjs"))
        .env("ELASTOS_CONTENT_SANDBOX_URL", format!("http://{addr}"))
        .env("ELASTOS_CONTENT_SANDBOX_CID", TEST_CIDV1)
        .env("ELASTOS_CONTENT_SANDBOX_COOKIE_NAME", cookie_name)
        .env("ELASTOS_CONTENT_SANDBOX_COOKIE_VALUE", authority.home_token)
        .env("ELASTOS_FRONTDOOR_FIXTURE", "1")
        .env("ELASTOS_FRONTDOOR_APP_TOKEN", assistant_token)
        .kill_on_drop(true);
    let result = tokio::time::timeout(std::time::Duration::from_secs(90), command.output()).await;
    let _ = shutdown_tx.send(());
    if tokio::time::timeout(std::time::Duration::from_secs(5), &mut server)
        .await
        .is_err()
    {
        server.abort();
        let _ = server.await;
        panic!("content sandbox gateway did not stop");
    }
    let output = result
        .expect("content sandbox browser timeout")
        .expect("start Node browser smoke");
    assert!(
        output.status.success(),
        "browser boundary failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
