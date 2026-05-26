use super::*;

#[test]
fn test_content_type_mapping() {
    assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
    assert_eq!(content_type("style.css"), "text/css");
    assert_eq!(content_type("app.js"), "application/javascript");
    assert_eq!(content_type("data.json"), "application/json");
    assert_eq!(
        content_type("manifest.webmanifest"),
        "application/manifest+json"
    );
    assert_eq!(content_type("README.md"), "text/markdown; charset=utf-8");
    assert_eq!(content_type("image.png"), "image/png");
    assert_eq!(content_type("photo.jpg"), "image/jpeg");
    assert_eq!(content_type("photo.jpeg"), "image/jpeg");
    assert_eq!(content_type("wallpaper.webp"), "image/webp");
    assert_eq!(content_type("icon.svg"), "image/svg+xml");
    assert_eq!(content_type("module.wasm"), "application/wasm");
    assert_eq!(content_type("unknown.xyz"), "application/octet-stream");
    assert_eq!(content_type("noext"), "application/octet-stream");
}

#[test]
fn test_validate_file_path() {
    assert!(validate_file_path("index.html").is_ok());
    assert!(validate_file_path("sub/dir/file.js").is_ok());
    assert!(validate_file_path("a.b.c.txt").is_ok());

    assert!(validate_file_path("../etc/passwd").is_err());
    assert!(validate_file_path("foo/../../etc/passwd").is_err());
    assert!(validate_file_path("/absolute/path").is_err());
    assert!(validate_file_path("foo\\bar").is_err());
    assert!(validate_file_path("\\windows\\path").is_err());
}

#[test]
fn test_validate_file_path_encoded() {
    assert!(validate_file_path("%2e%2e/etc/passwd").is_err());
    assert!(validate_file_path("%2E%2E/etc/passwd").is_err());
    assert!(validate_file_path("foo%2F..%2Fetc/passwd").is_err());
    assert!(validate_file_path("foo/%2e%2e/bar").is_err());
}

#[test]
fn test_advertised_gateway_urls_for_specific_host() {
    let urls = advertised_gateway_urls("77.42.19.31:18090");
    assert_eq!(urls, vec!["http://77.42.19.31:18090/"]);
}

#[test]
fn test_advertised_gateway_urls_for_wildcard_bind_starts_with_loopback() {
    let urls = advertised_gateway_urls("0.0.0.0:18090");
    assert_eq!(
        urls.first().map(String::as_str),
        Some("http://127.0.0.1:18090/")
    );
}

#[tokio::test]
async fn test_landing_page_200() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("ElastOS Gateway"));
}

#[tokio::test]
async fn test_root_serves_mywebsite_when_staged() {
    let dir = tempfile::tempdir().unwrap();
    let site_root = elastos_common::localhost::my_website_root_path(dir.path());
    std::fs::create_dir_all(&site_root).unwrap();
    std::fs::write(site_root.join("index.html"), "<html>home site</html>").unwrap();

    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("x-elastos-site-origin")
            .and_then(|v| v.to_str().ok()),
        Some("localhost://MyWebSite")
    );
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"<html>home site</html>");
}

#[tokio::test]
async fn test_healthz_200() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_invalid_cid_400() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/s/not-a-cid/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_cid_without_trailing_slash_redirects() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/s/{}", TEST_CIDV1))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert_eq!(location, format!("/s/{}/", TEST_CIDV1));
}

#[tokio::test]
async fn test_ipfs_cid_root_serves_cached_raw_file_without_redirect() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(format!("{}.raw", TEST_CIDV1)),
        b"raw-binary",
    )
    .unwrap();

    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/ipfs/{}", TEST_CIDV1))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert_eq!(ct, "application/octet-stream");
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"raw-binary");
}

#[tokio::test]
async fn test_ipfs_cid_root_serves_cached_directory_index_when_raw_file_missing() {
    let dir = tempfile::tempdir().unwrap();
    let cid_dir = dir.path().join(TEST_CIDV1);
    std::fs::create_dir_all(&cid_dir).unwrap();
    std::fs::write(cid_dir.join("index.html"), "<html>ok</html>").unwrap();

    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/ipfs/{}", TEST_CIDV1))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert_eq!(ct, "text/html; charset=utf-8");
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"<html>ok</html>");
}

#[tokio::test]
async fn test_cid_file_fetches_through_content_provider() {
    let dir = tempfile::tempdir().unwrap();
    let state = content_test_state(dir.path()).await;
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/s/{}/index.html", TEST_CIDV1))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert_eq!(ct, "text/html; charset=utf-8");
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"<html>content provider</html>");
}

#[tokio::test]
async fn test_ipfs_cid_root_without_provider_registry_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/ipfs/{}", TEST_CIDV1))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_traversal_400() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/s/{}/../etc/passwd", TEST_CIDV0))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_missing_file_404() {
    let dir = tempfile::tempdir().unwrap();
    // Pre-populate cache so we don't need IPFS
    let cid_dir = dir.path().join(TEST_CIDV1);
    std::fs::create_dir_all(&cid_dir).unwrap();
    std::fs::write(cid_dir.join("index.html"), "<html></html>").unwrap();

    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/s/{}/no-such-file.txt", TEST_CIDV1))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_release_head_200() {
    let dir = tempfile::tempdir().unwrap();
    let head = r#"{"payload":{"schema":"elastos.release.head/v1"}}"#;
    let publisher_root = publisher_release_head_path(dir.path());
    std::fs::create_dir_all(publisher_root.parent().unwrap()).unwrap();
    std::fs::write(publisher_root, head).unwrap();

    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/release-head.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert_eq!(ct, "application/json");
}

#[tokio::test]
async fn test_release_head_404() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/release-head.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_release_json_200() {
    let dir = tempfile::tempdir().unwrap();
    let release = r#"{"payload":{"schema":"elastos.release/v1"}}"#;
    let publisher_root = publisher_release_manifest_path(dir.path());
    std::fs::create_dir_all(publisher_root.parent().unwrap()).unwrap();
    std::fs::write(publisher_root, release).unwrap();

    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/release.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert_eq!(ct, "application/json");
}

#[tokio::test]
async fn test_install_sh_200() {
    let dir = tempfile::tempdir().unwrap();
    let install_path = publisher_install_script_path(dir.path());
    std::fs::create_dir_all(install_path.parent().unwrap()).unwrap();
    std::fs::write(install_path, "#!/bin/bash\necho hi").unwrap();

    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/install.sh")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert_eq!(ct, "text/x-shellscript");
}

#[tokio::test]
async fn test_install_sh_404() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/install.sh")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_artifact_file_200() {
    let dir = tempfile::tempdir().unwrap();
    let artifacts_dir = publisher_artifacts_path(dir.path());
    std::fs::create_dir_all(&artifacts_dir).unwrap();
    std::fs::write(artifacts_dir.join("components-linux-amd64.json"), "{}").unwrap();

    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/artifacts/components-linux-amd64.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert_eq!(ct, "application/json");
}

#[tokio::test]
async fn test_domain_binding_serves_bound_root() {
    let dir = tempfile::tempdir().unwrap();
    let public_site = dir.path().join("Public").join("docs");
    std::fs::create_dir_all(&public_site).unwrap();
    std::fs::write(public_site.join("index.html"), "<html>bound site</html>").unwrap();

    let binding_path = edge_binding_path(dir.path(), "docs.example.com");
    std::fs::create_dir_all(binding_path.parent().unwrap()).unwrap();
    std::fs::write(
        &binding_path,
        r#"{"domain":"docs.example.com","target":"localhost://Public/docs"}"#,
    )
    .unwrap();

    let state = test_state(dir.path());
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "docs.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("x-elastos-site-origin")
            .and_then(|v| v.to_str().ok()),
        Some("localhost://Public/docs")
    );
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"<html>bound site</html>");
}

#[tokio::test]
async fn test_site_head_document_and_headers() {
    let dir = tempfile::tempdir().unwrap();
    let site_root = elastos_common::localhost::my_website_root_path(dir.path());
    std::fs::create_dir_all(&site_root).unwrap();
    std::fs::write(site_root.join("index.html"), "<html>home site</html>").unwrap();
    let cached_bundle = dir.path().join(TEST_CIDV1);
    std::fs::create_dir_all(&cached_bundle).unwrap();
    std::fs::write(
        cached_bundle.join("index.html"),
        "<html>published bundle</html>",
    )
    .unwrap();

    let head_path = edge_site_head_path(dir.path(), MY_WEBSITE_URI);
    std::fs::create_dir_all(head_path.parent().unwrap()).unwrap();
    std::fs::write(
            &head_path,
            r#"{"payload":{"schema":"elastos.site.head.v1","target":"localhost://MyWebSite","bundle_cid":"bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi","release_name":"v1","channel_name":"live","content_digest":"sha256:abc123","entry_count":1,"total_bytes":21,"activated_at":123},"signature":"deadbeef","signer_did":"did:key:z6Mkexample"}"#,
        )
        .unwrap();

    let state = test_state(dir.path());
    let app = gateway_router(state);

    let root_resp = app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(root_resp.status(), StatusCode::OK);
    assert_eq!(
        root_resp
            .headers()
            .get("x-elastos-site-head-schema")
            .and_then(|v| v.to_str().ok()),
        Some("elastos.site.head.v1")
    );
    assert_eq!(
        root_resp
            .headers()
            .get("x-elastos-site-head-digest")
            .and_then(|v| v.to_str().ok()),
        Some("sha256:abc123")
    );
    assert_eq!(
        root_resp
            .headers()
            .get("x-elastos-site-head-cid")
            .and_then(|v| v.to_str().ok()),
        Some(TEST_CIDV1)
    );
    assert_eq!(
        root_resp
            .headers()
            .get("x-elastos-site-head-release")
            .and_then(|v| v.to_str().ok()),
        Some("v1")
    );
    assert_eq!(
        root_resp
            .headers()
            .get("x-elastos-site-head-channel")
            .and_then(|v| v.to_str().ok()),
        Some("live")
    );

    let head_resp = app
        .oneshot(
            Request::builder()
                .uri("/.well-known/elastos/site-head.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(head_resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(head_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("\"schema\":\"elastos.site.head.v1\""));
    assert!(text.contains("\"target\":\"localhost://MyWebSite\""));
    assert!(text.contains(&format!("\"bundle_cid\":\"{}\"", TEST_CIDV1)));
    assert!(text.contains("\"release_name\":\"v1\""));
    assert!(text.contains("\"channel_name\":\"live\""));
}

#[tokio::test]
async fn test_active_site_head_prefers_bundle_cid() {
    let dir = tempfile::tempdir().unwrap();
    let site_root = elastos_common::localhost::my_website_root_path(dir.path());
    std::fs::create_dir_all(&site_root).unwrap();
    std::fs::write(site_root.join("index.html"), "<html>working tree</html>").unwrap();

    let cached_bundle = dir.path().join(TEST_CIDV1);
    std::fs::create_dir_all(&cached_bundle).unwrap();
    std::fs::write(
        cached_bundle.join("index.html"),
        "<html>published bundle</html>",
    )
    .unwrap();

    let head_path = edge_site_head_path(dir.path(), MY_WEBSITE_URI);
    std::fs::create_dir_all(head_path.parent().unwrap()).unwrap();
    std::fs::write(
            &head_path,
            format!(
                r#"{{"payload":{{"schema":"elastos.site.head.v1","target":"localhost://MyWebSite","bundle_cid":"{}","release_name":"v2","channel_name":"live","content_digest":"sha256:abc123","entry_count":1,"total_bytes":28,"activated_at":123}},"signature":"deadbeef","signer_did":"did:key:z6Mkexample"}}"#,
                TEST_CIDV1
            ),
        )
        .unwrap();

    let app = gateway_router(test_state(dir.path()));
    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("x-elastos-site-head-cid")
            .and_then(|v| v.to_str().ok()),
        Some(TEST_CIDV1)
    );
    assert_eq!(
        resp.headers()
            .get("x-elastos-site-head-release")
            .and_then(|v| v.to_str().ok()),
        Some("v2")
    );
    assert_eq!(
        resp.headers()
            .get("x-elastos-site-head-channel")
            .and_then(|v| v.to_str().ok()),
        Some("live")
    );
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"<html>published bundle</html>");
}
