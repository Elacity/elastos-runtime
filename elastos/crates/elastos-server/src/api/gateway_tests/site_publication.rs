use super::*;
use elastos_runtime::signature::{generate_keypair, SigningKey};

// This named fixture owns a disposable, memory-only key. Files contain only
// signed public metadata and harmless candidate bytes; candidates stay inert.
struct SignedGatewayPublication {
    _temporary: tempfile::TempDir,
    base: std::path::PathBuf,
    root: std::path::PathBuf,
    installer: Vec<u8>,
    release: Vec<u8>,
    head: Vec<u8>,
    binary: Vec<u8>,
}

fn gateway_fixture_digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn gateway_fixture_cid(bytes: &[u8]) -> String {
    let hash = cid::multihash::Multihash::<64>::wrap(0x12, &Sha256::digest(bytes)).unwrap();
    cid::Cid::new_v1(0x55, hash).to_string()
}

fn gateway_fixture_descriptor(bytes: &[u8]) -> Value {
    json!({"cid":gateway_fixture_cid(bytes),"sha256":gateway_fixture_digest(bytes),"size":bytes.len()})
}

fn gateway_fixture_envelope(key: &SigningKey, domain: &str, payload: Value) -> Vec<u8> {
    let bytes = serde_json::to_vec(&payload).unwrap();
    let (signature, signer_did) = crate::crypto::domain_separated_sign(key, domain, &bytes);
    serde_json::to_vec(&json!({"payload":payload,"signature":signature,"signer_did":signer_did}))
        .unwrap()
}

impl SignedGatewayPublication {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let root = elastos_common::localhost::publisher_root_path(&base);
        let artifacts = root.join("artifacts");
        std::fs::create_dir_all(&artifacts).unwrap();
        let (key, _) = generate_keypair();
        let did = crate::crypto::encode_signing_key_did(&key);
        let binary =
            b"#!/bin/sh\ncat /custodian/unopened-signing.pem > candidate-executed\n".to_vec();
        let installer = b"#!/bin/sh\n# Frozen public bytes: __PUBLISHER_GATEWAY__\n".to_vec();
        let support = b"public qualified support bytes";
        let mut support_ref = gateway_fixture_descriptor(support);
        support_ref["release_path"] = json!("home.tar.gz");
        support_ref["checksum"] = json!(format!("sha256:{}", gateway_fixture_digest(support)));
        support_ref.as_object_mut().unwrap().remove("sha256");
        let components =
            serde_json::to_vec(&json!({"schema":"elastos.components/v1","capsules":{},
            "external":{"home":{"platforms":{"*":support_ref}}}}))
            .unwrap();
        let release = gateway_fixture_envelope(
            &key,
            "elastos.release.v1",
            json!({
            "schema":"elastos.release/v1","version":"0.7.1","channel":"canary",
            "source":{"commit":"a".repeat(40),"tree":"b".repeat(40)},"released_at":1,
            "prev_release_cid":null,"installer_sha256":gateway_fixture_digest(&installer),
            "platforms":{"aarch64-darwin":{"binary":gateway_fixture_descriptor(&binary),
                "components":gateway_fixture_descriptor(&components)}}}),
        );
        let head = gateway_fixture_envelope(
            &key,
            "elastos.release.head.v1",
            json!({
            "schema":"elastos.release.head/v1","version":"0.7.1","channel":"canary","signer_did":did,
            "updated_at":2,"prev_head_cid":null,"latest_release_cid":gateway_fixture_cid(&release),
            "release_sha256":gateway_fixture_digest(&release)}),
        );
        for (name, bytes) in [
            ("install.sh", &installer),
            ("release.json", &release),
            ("release-head.json", &head),
        ] {
            std::fs::write(root.join(name), bytes).unwrap();
        }
        for (name, bytes) in [
            ("elastos-aarch64-darwin", binary.as_slice()),
            ("components-aarch64-darwin.json", components.as_slice()),
            ("home.tar.gz", &support[..]),
        ] {
            std::fs::write(artifacts.join(name), bytes).unwrap();
        }
        std::fs::write(root.join("publish-state.json"),serde_json::to_vec(&json!({
            "publisher_did":did,"last_release_cid":gateway_fixture_cid(&release),
            "last_head_cid":gateway_fixture_cid(&head),"last_version":"0.7.1","last_published_at":2})).unwrap()).unwrap();
        Self {
            _temporary: temporary,
            base,
            root,
            installer,
            release,
            head,
            binary,
        }
    }

    fn change_receipt(&self, field: &str, value: Value) {
        let path = self.root.join("publish-state.json");
        let mut receipt: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        receipt[field] = value;
        std::fs::write(path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    }

    async fn response(&self, path: &str) -> Response {
        gateway_router(test_state(&self.base))
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
}

#[test]
fn test_content_type_mapping() {
    assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
    assert_eq!(content_type("style.css"), "text/css");
    assert_eq!(content_type("app.js"), "application/javascript");
    assert_eq!(content_type("module.mjs"), "application/javascript");
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
async fn test_content_cid_root_fetches_raw_file_through_content_provider() {
    let dir = tempfile::tempdir().unwrap();
    let state = content_test_state(dir.path()).await;
    let app = gateway_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/content/{}", TEST_CIDV1))
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
    assert_eq!(&body[..], b"raw-content-provider-bytes");
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
async fn test_signed_release_metadata_and_artifacts_200_exact_bytes() {
    let fixture = SignedGatewayPublication::new();
    for (path, expected, media_type) in [
        (
            "/release-head.json",
            fixture.head.as_slice(),
            "application/json",
        ),
        (
            "/release.json",
            fixture.release.as_slice(),
            "application/json",
        ),
        (
            "/install.sh",
            fixture.installer.as_slice(),
            "text/x-shellscript",
        ),
        (
            "/artifacts/elastos-aarch64-darwin",
            fixture.binary.as_slice(),
            "application/octet-stream",
        ),
    ] {
        let response = fixture.response(path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers().get("content-type").unwrap(), media_type);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), expected, "{path}");
    }
    assert!(!fixture.root.join("candidate-executed").exists());
}

#[tokio::test]
async fn test_missing_release_publication_404() {
    let temporary = tempfile::tempdir().unwrap();
    for path in [
        "/release-head.json",
        "/release.json",
        "/install.sh",
        "/artifacts/elastos-aarch64-darwin",
    ] {
        let response = gateway_router(test_state(temporary.path()))
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn test_unsigned_release_publication_503() {
    let temporary = tempfile::tempdir().unwrap();
    let path = elastos_common::localhost::publisher_release_head_path(temporary.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, br#"{"payload":{"schema":"elastos.release.head/v1"}}"#).unwrap();
    let response = gateway_router(test_state(temporary.path()))
        .oneshot(
            Request::builder()
                .uri("/release-head.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn test_install_script_frozen_across_host_and_forwarded_headers() {
    let fixture = SignedGatewayPublication::new();
    // Call the handler directly so this test concerns signed byte ownership,
    // while gateway origin admission keeps its own route-level test coverage.
    for host in [
        "localhost:61180",
        "mirror.example.invalid",
        "hostile.example.invalid",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert("host", host.parse().unwrap());
        headers.insert(
            "x-forwarded-host",
            "changed.example.invalid".parse().unwrap(),
        );
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        let response = super::super::gateway_site::serve_install_script(
            AxumState(test_state(&fixture.base)),
            Extension(super::super::gateway_site::ReleaseReadGate::new()),
            headers,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), fixture.installer.as_slice());
    }
}

fn publication_test_router(
    fixture: &SignedGatewayPublication,
    gate: super::super::gateway_site::ReleaseReadGate,
) -> Router {
    use super::super::gateway_site::{
        healthz, serve_artifact_file, serve_install_script, serve_release_head,
        serve_release_manifest,
    };
    Router::new()
        .route("/release-head.json", get(serve_release_head))
        .route("/release.json", get(serve_release_manifest))
        .route("/install.sh", get(serve_install_script))
        .route("/artifacts/*path", get(serve_artifact_file))
        .route("/healthz", get(healthz))
        .layer(Extension(gate))
        .with_state(test_state(&fixture.base))
}

#[tokio::test]
async fn test_concurrent_release_reads_wait_health_works_and_scan_once() {
    use super::super::gateway_site::ReleaseReadGate;
    let fixture = SignedGatewayPublication::new();
    let gate = ReleaseReadGate::new();
    let app = publication_test_router(&fixture, gate.clone());
    let permit = gate.permit.clone().try_acquire_owned().unwrap();
    let mut requests = Vec::new();
    for path in [
        "/release-head.json",
        "/release.json",
        "/install.sh",
        "/artifacts/elastos-aarch64-darwin",
    ] {
        let router = app.clone();
        requests.push(tokio::spawn(async move {
            router
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap()
        }));
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut requests[0])
            .await
            .is_err()
    );
    assert!(requests.iter().all(|request| !request.is_finished()));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    drop(permit);
    for (request, expected) in requests.into_iter().zip([
        &fixture.head,
        &fixture.release,
        &fixture.installer,
        &fixture.binary,
    ]) {
        let response = tokio::time::timeout(std::time::Duration::from_secs(5), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), expected.as_slice());
    }
    for _ in 0..3 {
        assert_eq!(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/release-head.json")
                        .body(Body::empty())
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(gate.permit.available_permits(), 1);
    assert_eq!(
        gate.admissions.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[tokio::test]
async fn test_cached_publication_tamper_refused_until_new_receipt_commit() {
    use super::super::gateway_site::ReleaseReadGate;
    for name in [
        "release.json",
        "install.sh",
        "artifacts/elastos-aarch64-darwin",
        "artifacts/home.tar.gz",
    ] {
        let fixture = SignedGatewayPublication::new();
        let gate = ReleaseReadGate::new();
        let app = publication_test_router(&fixture, gate.clone());
        let request = || {
            Request::builder()
                .uri("/release-head.json")
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(
            app.clone().oneshot(request()).await.unwrap().status(),
            StatusCode::OK
        );
        let path = fixture.root.join(name);
        let original = std::fs::read(&path).unwrap();
        let mut tampered = original.clone();
        tampered[0] ^= 1;
        std::fs::write(&path, tampered).unwrap();
        for route in [
            "/release-head.json",
            "/release.json",
            "/install.sh",
            "/artifacts/elastos-aarch64-darwin",
        ] {
            assert_eq!(
                app.clone()
                    .oneshot(Request::builder().uri(route).body(Body::empty()).unwrap())
                    .await
                    .unwrap()
                    .status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{name} {route}"
            );
        }
        std::fs::write(path, original).unwrap();
        assert_eq!(
            app.clone().oneshot(request()).await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            gate.admissions.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        let receipt = fixture.root.join("publish-state.json");
        let next = fixture.root.join("next-receipt.json");
        std::fs::write(&next, std::fs::read(&receipt).unwrap()).unwrap();
        std::fs::rename(next, receipt).unwrap();
        assert_eq!(
            app.clone().oneshot(request()).await.unwrap().status(),
            StatusCode::OK,
            "{name}"
        );
        assert_eq!(
            gate.admissions.load(std::sync::atomic::Ordering::Relaxed),
            2
        );
    }
}

#[tokio::test]
async fn test_restored_publication_reactivates_after_backup_cleanup_and_head_commit() {
    use super::super::gateway_site::ReleaseReadGate;
    let fixture = SignedGatewayPublication::new();
    let gate = ReleaseReadGate::new();
    let app = publication_test_router(&fixture, gate.clone());
    let request = || {
        Request::builder()
            .uri("/release-head.json")
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        app.clone().oneshot(request()).await.unwrap().status(),
        StatusCode::OK
    );
    let binary = fixture.root.join("artifacts/elastos-aarch64-darwin");
    let backup = fixture.base.join("rollback-binary");
    std::fs::hard_link(&binary, &backup).unwrap();
    assert_eq!(
        app.clone().oneshot(request()).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    std::fs::remove_file(backup).unwrap();
    assert_eq!(
        app.clone().oneshot(request()).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let head = fixture.root.join("release-head.json");
    let recovered = fixture.root.join("recovered-head.json");
    std::fs::write(&recovered, &fixture.head).unwrap();
    std::fs::rename(recovered, head).unwrap();
    for (route, expected) in [
        ("/release-head.json", &fixture.head),
        ("/install.sh", &fixture.installer),
        ("/artifacts/elastos-aarch64-darwin", &fixture.binary),
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(route).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), expected.as_slice());
    }
    assert_eq!(
        gate.admissions.load(std::sync::atomic::Ordering::Relaxed),
        2
    );
}

#[tokio::test]
async fn test_receipt_before_head_refusal_recovers_on_final_head_commit() {
    use super::super::gateway_site::ReleaseReadGate;
    let old = SignedGatewayPublication::new();
    let new = SignedGatewayPublication::new();
    let gate = ReleaseReadGate::new();
    let app = publication_test_router(&old, gate.clone());
    let request = || {
        Request::builder()
            .uri("/release-head.json")
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        app.clone().oneshot(request()).await.unwrap().status(),
        StatusCode::OK
    );
    for name in [
        "release.json",
        "install.sh",
        "publish-state.json",
        "artifacts/elastos-aarch64-darwin",
        "artifacts/components-aarch64-darwin.json",
        "artifacts/home.tar.gz",
    ] {
        std::fs::copy(new.root.join(name), old.root.join(name)).unwrap();
    }
    for _ in 0..2 {
        assert_eq!(
            app.clone().oneshot(request()).await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    assert_eq!(
        gate.admissions.load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    let staged_head = old.root.join("next-head.json");
    std::fs::write(&staged_head, &new.head).unwrap();
    std::fs::rename(staged_head, old.root.join("release-head.json")).unwrap();
    let response = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), new.head.as_slice());
    assert_eq!(
        gate.admissions.load(std::sync::atomic::Ordering::Relaxed),
        3
    );
}

#[tokio::test]
async fn test_refused_publication_is_not_rescanned_for_same_receipt() {
    use super::super::gateway_site::ReleaseReadGate;
    let fixture = SignedGatewayPublication::new();
    std::fs::write(fixture.root.join("artifacts/home.tar.gz"), b"tampered").unwrap();
    let gate = ReleaseReadGate::new();
    let app = publication_test_router(&fixture, gate.clone());
    for _ in 0..3 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/release-head.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
    assert_eq!(
        gate.admissions.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[tokio::test]
async fn test_wrong_public_pin_receipt_cid_version_and_mixed_receipt_503() {
    for field in [
        "publisher_did",
        "last_release_cid",
        "last_head_cid",
        "last_version",
    ] {
        let fixture = SignedGatewayPublication::new();
        let (other, _) = generate_keypair();
        let value = match field {
            "publisher_did" => json!(crate::crypto::encode_signing_key_did(&other)),
            "last_version" => json!("0.7.2"),
            _ => json!(gateway_fixture_cid(b"different public metadata")),
        };
        fixture.change_receipt(field, value);
        for path in [
            "/release-head.json",
            "/release.json",
            "/install.sh",
            "/artifacts/home.tar.gz",
        ] {
            assert_eq!(
                fixture.response(path).await.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{field} {path}"
            );
        }
    }
    let old = SignedGatewayPublication::new();
    let new = SignedGatewayPublication::new();
    std::fs::copy(
        old.root.join("publish-state.json"),
        new.root.join("publish-state.json"),
    )
    .unwrap();
    assert_eq!(
        new.response("/install.sh").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let fixture = SignedGatewayPublication::new();
    fixture.change_receipt("last_head_cid", json!("malformed-cid"));
    assert_eq!(
        fixture.response("/release-head.json").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn test_invalid_signature_metadata_and_artifact_tamper_503() {
    for name in [
        "release-head.json",
        "release.json",
        "install.sh",
        "artifacts/elastos-aarch64-darwin",
        "artifacts/home.tar.gz",
    ] {
        let fixture = SignedGatewayPublication::new();
        std::fs::write(fixture.root.join(name), b"tampered public bytes").unwrap();
        for path in [
            "/release-head.json",
            "/release.json",
            "/install.sh",
            "/artifacts/elastos-aarch64-darwin",
        ] {
            let response = fixture.response(path).await;
            assert_eq!(
                response.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{name} {path}"
            );
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(body.as_ref(), b"Release publication unavailable");
        }
    }
    let fixture = SignedGatewayPublication::new();
    let mut head: Value = serde_json::from_slice(&fixture.head).unwrap();
    head["signature"] = json!("00".repeat(64));
    std::fs::write(
        fixture.root.join("release-head.json"),
        serde_json::to_vec(&head).unwrap(),
    )
    .unwrap();
    assert_eq!(
        fixture.response("/release-head.json").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn test_unadvertised_and_stale_artifacts_404_and_nested_paths_400() {
    let fixture = SignedGatewayPublication::new();
    std::fs::write(fixture.root.join("artifacts/old-platform"), b"stale bytes").unwrap();
    for path in [
        "/artifacts/old-platform",
        "/artifacts/unadvertised",
        "/artifacts/install.sh",
    ] {
        assert_eq!(
            fixture.response(path).await.status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    for path in [
        "/artifacts/sub/file",
        "/artifacts/hidden..file",
        "/artifacts/.hidden",
    ] {
        assert_eq!(
            fixture.response(path).await.status(),
            StatusCode::BAD_REQUEST,
            "{path}"
        );
    }
}

#[tokio::test]
async fn test_linked_receipt_metadata_and_artifacts_503() {
    use std::os::unix::fs::symlink;
    for name in [
        "publish-state.json",
        "release.json",
        "install.sh",
        "artifacts/home.tar.gz",
    ] {
        let fixture = SignedGatewayPublication::new();
        let path = fixture.root.join(name);
        let marker = fixture.base.join("public-marker");
        std::fs::rename(&path, &marker).unwrap();
        symlink(&marker, &path).unwrap();
        assert_eq!(
            fixture.response("/install.sh").await.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{name}"
        );
    }
    let fixture = SignedGatewayPublication::new();
    std::fs::hard_link(
        fixture.root.join("publish-state.json"),
        fixture.base.join("receipt-link"),
    )
    .unwrap();
    assert_eq!(
        fixture.response("/release.json").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    for directory in ["artifacts", "publisher"] {
        let fixture = SignedGatewayPublication::new();
        let path = if directory == "publisher" {
            fixture.root.clone()
        } else {
            fixture.root.join(directory)
        };
        let retained = fixture.base.join("retained-public-directory");
        std::fs::rename(&path, &retained).unwrap();
        symlink(&retained, &path).unwrap();
        assert_eq!(
            fixture.response("/install.sh").await.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{directory}"
        );
    }
}

#[tokio::test]
async fn test_unsafe_or_oversized_receipt_503() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = SignedGatewayPublication::new();
    let path = fixture.root.join("publish-state.json");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(
        fixture.response("/release.json").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let fixture = SignedGatewayPublication::new();
    std::fs::write(
        fixture.root.join("publish-state.json"),
        vec![b' '; 64 * 1024 + 1],
    )
    .unwrap();
    assert_eq!(
        fixture.response("/install.sh").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn test_domain_binding_serves_bound_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "gateway_allowed_hosts = [\"docs.example.com\"]\n",
    )
    .unwrap();
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
