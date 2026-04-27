use super::*;
use crate::sources::{save_trusted_sources, TrustedSource, TrustedSourcesConfig};
use axum::body::Body;
use axum::extract::{Path as AxumPath, State as AxumState};
use axum::http::{
    header::{CONTENT_TYPE, COOKIE},
    HeaderMap, Request,
};
use axum::routing::{get, post};
use axum::Json as AxumJson;
use ed25519_dalek::{Signer as _, Verifier as _};
use serde_json::json;
use std::collections::{BTreeSet, HashMap};
use tokio::net::TcpListener;
use tokio::sync::Mutex as TokioMutex;
use tower::ServiceExt;

// Real CIDs that pass cid crate validation
const TEST_CIDV0: &str = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";
const TEST_CIDV1: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";
const GBA_EMULATOR_CAPSULE_ID: &str = "gba-emulator";

fn test_state(cache_dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    GatewayState {
        provider_registry: None,
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn documents_test_state(cache_dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register(Arc::new(crate::documents::DocumentsProvider::new(
            cache_dir.to_path_buf(),
            Arc::downgrade(&registry),
        )))
        .await;
    GatewayState {
        provider_registry: Some(registry),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

fn write_test_capsule_manifest(data_dir: &std::path::Path, name: &str) {
    let role = if name == GBA_EMULATOR_CAPSULE_ID {
        "viewer"
    } else {
        "app"
    };
    write_test_browser_capsule(data_dir, name, role, "Installed test capsule", None);
}

fn write_test_browser_capsule(
    data_dir: &std::path::Path,
    name: &str,
    role: &str,
    description: &str,
    index_html: Option<&str>,
) {
    let capsule_dir = data_dir.join("capsules").join(name);
    let browser_dir = capsule_dir.join("browser");
    std::fs::create_dir_all(&browser_dir).unwrap();
    std::fs::write(
        capsule_dir.join("capsule.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": "elastos.capsule/v1",
            "name": name,
            "version": "0.1.0",
            "description": description,
            "author": "elastos",
            "role": role,
            "type": "wasm",
            "entrypoint": format!("{name}.wasm")
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        browser_dir.join("index.html"),
        index_html.unwrap_or("<!doctype html><title>Test Capsule</title>"),
    )
    .unwrap();
}

fn write_test_static_capsule(
    data_dir: &std::path::Path,
    name: &str,
    role: &str,
    description: &str,
    index_html: &str,
) {
    let capsule_dir = data_dir.join("capsules").join(name);
    std::fs::create_dir_all(&capsule_dir).unwrap();
    std::fs::write(
        capsule_dir.join("capsule.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": "elastos.capsule/v1",
            "name": name,
            "version": "0.1.0",
            "description": description,
            "author": "elastos",
            "role": role,
            "type": "data",
            "entrypoint": "index.html"
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(capsule_dir.join("index.html"), index_html).unwrap();
}

fn seed_test_browser_capsules(data_dir: &std::path::Path) {
    write_test_browser_capsule(
        data_dir,
        HOME_CAPSULE_ID,
        "app",
        "Test Home capsule",
        Some(r#"<!doctype html><title>Home · ElastOS</title><script src="./shell.js"></script>"#),
    );
    std::fs::write(
        data_dir
            .join("capsules")
            .join(HOME_CAPSULE_ID)
            .join("browser")
            .join("shell.js"),
        "window.__TEST_HOME__ = true;",
    )
    .unwrap();

    write_test_browser_capsule(
        data_dir,
        SYSTEM_CAPSULE_ID,
        "app",
        "Test System capsule",
        Some("<!doctype html><title>System</title>"),
    );
    write_test_static_capsule(
        data_dir,
        DOCUMENTS_CAPSULE_ID,
        "app",
        "Test Documents capsule",
        "<!doctype html><title>Documents</title>",
    );
    write_test_static_capsule(
        data_dir,
        LIBRARY_CAPSULE_ID,
        "app",
        "Test Library capsule",
        "<!doctype html><title>Library</title>",
    );
    write_test_static_capsule(
        data_dir,
        INBOX_CAPSULE_ID,
        "app",
        "Test Inbox capsule",
        "<!doctype html><title>Inbox</title>",
    );
    write_test_browser_capsule(
        data_dir,
        CHAT_ROOM_CAPSULE_ID,
        "app",
        "Test Chat Room capsule",
        Some("<!doctype html><title>Chat Room</title>Chat Room"),
    );
    std::fs::write(
        data_dir
            .join("capsules")
            .join(CHAT_ROOM_CAPSULE_ID)
            .join("browser")
            .join("chat_room_ui_bg.wasm"),
        b"\0asm",
    )
    .unwrap();

    write_test_browser_capsule(
        data_dir,
        GBA_EMULATOR_CAPSULE_ID,
        "viewer",
        "Test GBA emulator capsule",
        Some("<!doctype html><title>GBA Emulator</title>"),
    );
    write_test_viewer_capsule(
        data_dir,
        "gba-ucity",
        GBA_EMULATOR_CAPSULE_ID,
        "ucity.gba",
        "uCity",
    );
}

fn write_test_viewer_capsule(
    data_dir: &std::path::Path,
    name: &str,
    viewer: &str,
    entrypoint: &str,
    description: &str,
) {
    let capsule_dir = data_dir.join("capsules").join(name);
    std::fs::create_dir_all(&capsule_dir).unwrap();
    std::fs::write(
        capsule_dir.join("capsule.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": "elastos.capsule/v1",
            "name": name,
            "version": "0.1.0",
            "description": description,
            "author": "elastos",
            "role": "content",
            "type": "data",
            "entrypoint": entrypoint,
            "viewer": viewer,
            "permissions": {
                "storage": ["localhost://Users/self/.AppData/LocalHost/GBA/test/*"]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(capsule_dir.join(entrypoint), "rom-data").unwrap();
}

fn room_cookie_header(response: &Response) -> String {
    response
        .headers()
        .get(SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_string)
        .expect("room session cookie header")
}

fn browser_cookie_header(response: &Response) -> String {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            value
                .strip_prefix(&format!("{BROWSER_SESSION_COOKIE}="))
                .map(|_| value.split(';').next().unwrap_or_default().to_string())
        })
        .expect("browser session cookie header")
}

fn browser_request_cookie_header(response: &Response) -> String {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            value
                .strip_prefix("browser-session-request=")
                .map(|_| value.split(';').next().unwrap_or_default().to_string())
        })
        .expect("browser access request cookie header")
}

fn home_app_token(data_dir: &std::path::Path) -> String {
    issue_home_launch_token(data_dir, HOME_CAPSULE_ID).unwrap()
}

fn system_app_token(data_dir: &std::path::Path) -> String {
    issue_home_launch_token(data_dir, SYSTEM_CAPSULE_ID).unwrap()
}

#[derive(Default)]
struct FakePeerBus {
    topic_members: HashMap<String, BTreeSet<String>>,
    topic_messages: HashMap<String, Vec<serde_json::Value>>,
    cursors: HashMap<(String, String, String), usize>,
}

#[derive(Clone)]
struct FakeRuntimeState {
    did: String,
    signing_key: ed25519_dalek::SigningKey,
    attach_secret: String,
    peer_id: String,
    bus: Arc<TokioMutex<FakePeerBus>>,
    audit_events: Arc<TokioMutex<Vec<elastos_runtime::primitives::audit::AuditEvent>>>,
}

struct FakeRuntimeHandle {
    api_url: String,
    _task: tokio::task::JoinHandle<()>,
}

fn verifying_key_from_did(did: &str) -> Option<ed25519_dalek::VerifyingKey> {
    let multibase = did.strip_prefix("did:key:z")?;
    let bytes = bs58::decode(multibase).into_vec().ok()?;
    if bytes.len() != 34 || bytes[0] != 0xed || bytes[1] != 0x01 {
        return None;
    }
    let key_bytes: [u8; 32] = bytes[2..34].try_into().ok()?;
    ed25519_dalek::VerifyingKey::from_bytes(&key_bytes).ok()
}

async fn start_fake_runtime(
    data_dir: &std::path::Path,
    bus: Arc<TokioMutex<FakePeerBus>>,
    peer_id: &str,
) -> FakeRuntimeHandle {
    let (signing_key, did) = elastos_identity::load_or_create_did(data_dir).unwrap();
    let state = FakeRuntimeState {
        did,
        signing_key,
        attach_secret: format!("attach-{peer_id}"),
        peer_id: peer_id.to_string(),
        bus,
        audit_events: Arc::new(TokioMutex::new(vec![
            elastos_runtime::primitives::audit::AuditEvent::RuntimeStart {
                timestamp: elastos_common::SecureTimestamp::now(),
                version: env!("ELASTOS_VERSION").to_string(),
            },
        ])),
    };
    let app = Router::new()
        .route("/api/auth/attach", post(fake_runtime_attach))
        .route("/api/health", get(fake_runtime_health))
        .route(
            "/api/capability/request",
            post(fake_runtime_capability_request),
        )
        .route(
            "/api/capsules",
            get(fake_runtime_list_capsules).post(fake_runtime_launch_capsule),
        )
        .route("/api/audit", get(fake_runtime_audit_log))
        .route("/api/provider/:scheme/:op", post(fake_runtime_provider))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let api_url = format!("http://{}", addr);
    std::fs::write(
        data_dir.join("runtime-coords.json"),
        serde_json::to_vec_pretty(&json!({
            "api_url": api_url,
            "attach_secret": state.attach_secret,
            "pid": std::process::id(),
            "runtime_kind": crate::runtime_control::RUNTIME_KIND_OPERATOR,
            "binary_sha256": "",
            "policy_sha256": "",
        }))
        .unwrap(),
    )
    .unwrap();
    FakeRuntimeHandle {
        api_url,
        _task: task,
    }
}

async fn fake_runtime_attach(
    AxumState(state): AxumState<FakeRuntimeState>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> Response {
    let secret = body
        .get("secret")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if secret != state.attach_secret {
        return (StatusCode::FORBIDDEN, "bad attach secret").into_response();
    }
    let scope = body
        .get("scope")
        .and_then(|value| value.as_str())
        .unwrap_or("client");
    AxumJson(json!({
        "token": format!("{scope}-{}", state.peer_id),
    }))
    .into_response()
}

async fn fake_runtime_health() -> Response {
    AxumJson(json!({ "version": env!("ELASTOS_VERSION") })).into_response()
}

fn fake_runtime_has_home_token(headers: &HeaderMap, state: &FakeRuntimeState) -> bool {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(|value| value == format!("Bearer shell-{}", state.peer_id))
        .unwrap_or(false)
}

async fn fake_runtime_list_capsules(
    headers: HeaderMap,
    AxumState(state): AxumState<FakeRuntimeState>,
) -> Response {
    if !fake_runtime_has_home_token(&headers, &state) {
        return (
            StatusCode::FORBIDDEN,
            "This endpoint requires shell privileges",
        )
            .into_response();
    }
    AxumJson(json!({ "capsules": [] })).into_response()
}

async fn fake_runtime_launch_capsule(
    headers: HeaderMap,
    AxumState(state): AxumState<FakeRuntimeState>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> Response {
    if !fake_runtime_has_home_token(&headers, &state) {
        return (
            StatusCode::FORBIDDEN,
            "This endpoint requires shell privileges",
        )
            .into_response();
    }
    let path = body
        .get("path")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let capsule_name = std::path::Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("unknown");
    state.audit_events.lock().await.push(
        elastos_runtime::primitives::audit::AuditEvent::CapsuleLaunch {
            timestamp: elastos_common::SecureTimestamp::now(),
            capsule_id: format!("wasm-{}-instance", capsule_name),
            capsule_name: capsule_name.to_string(),
            cid: None,
            trust_level: elastos_runtime::primitives::audit::TrustLevel::Untrusted,
        },
    );
    AxumJson(json!({
        "id": format!("wasm-{}-instance", capsule_name),
        "name": capsule_name,
        "status": "running",
    }))
    .into_response()
}

async fn fake_runtime_audit_log(AxumState(state): AxumState<FakeRuntimeState>) -> Response {
    let events = state.audit_events.lock().await.clone();
    AxumJson(json!({
        "events": events,
        "total_in_memory": events.len(),
        "current_epoch": 0,
    }))
    .into_response()
}

async fn fake_runtime_capability_request(
    AxumState(state): AxumState<FakeRuntimeState>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> Response {
    let resource = body
        .get("resource")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let token = if resource.starts_with("elastos://did/") {
        format!("did-cap-{}", state.peer_id)
    } else if resource.starts_with("elastos://peer/") {
        format!("peer-cap-{}", state.peer_id)
    } else {
        format!("cap-{}", state.peer_id)
    };
    AxumJson(json!({ "token": token })).into_response()
}

async fn fake_runtime_provider(
    AxumPath((scheme, op)): AxumPath<(String, String)>,
    AxumState(state): AxumState<FakeRuntimeState>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> Response {
    match (scheme.as_str(), op.as_str()) {
        ("did", "get_did") => AxumJson(json!({
            "status": "ok",
            "data": { "did": state.did }
        }))
        .into_response(),
        ("did", "sign") => {
            let data = body
                .get("data")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let Ok(bytes) = hex::decode(data) else {
                return AxumJson(json!({"status":"error","message":"invalid hex payload"}))
                    .into_response();
            };
            let signature = state.signing_key.sign(&bytes);
            AxumJson(json!({
                "status": "ok",
                "data": { "signature": hex::encode(signature.to_bytes()) }
            }))
            .into_response()
        }
        ("did", "verify") => {
            let did = body
                .get("did")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let data = body
                .get("data")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let signature = body
                .get("signature")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let valid = {
                let Ok(bytes) = hex::decode(data) else {
                    return AxumJson(json!({"status":"error","message":"invalid hex payload"}))
                        .into_response();
                };
                let Ok(sig_bytes) = hex::decode(signature) else {
                    return AxumJson(json!({"status":"error","message":"invalid signature"}))
                        .into_response();
                };
                let Ok(sig) = ed25519_dalek::Signature::try_from(sig_bytes.as_slice()) else {
                    return AxumJson(json!({"status":"error","message":"invalid signature"}))
                        .into_response();
                };
                verifying_key_from_did(did)
                    .map(|key| key.verify(&bytes, &sig).is_ok())
                    .unwrap_or(false)
            };
            AxumJson(json!({
                "status": "ok",
                "data": { "valid": valid }
            }))
            .into_response()
        }
        ("peer", "gossip_join") => {
            let topic = body
                .get("topic")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let mut bus = state.bus.lock().await;
            bus.topic_members
                .entry(topic.to_string())
                .or_default()
                .insert(state.peer_id.clone());
            AxumJson(json!({
                "status": "ok",
                "data": { "topic": topic }
            }))
            .into_response()
        }
        ("peer", "list_topic_peers") => {
            let topic = body
                .get("topic")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let bus = state.bus.lock().await;
            let peers = bus
                .topic_members
                .get(topic)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|peer| peer != &state.peer_id)
                .collect::<Vec<_>>();
            AxumJson(json!({
                "status": "ok",
                "data": { "topic": topic, "peers": peers }
            }))
            .into_response()
        }
        ("peer", "gossip_send") => {
            let topic = body
                .get("topic")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let mut bus = state.bus.lock().await;
            let peers = bus.topic_members.get(topic).cloned().unwrap_or_default();
            bus.topic_messages
                .entry(topic.to_string())
                .or_default()
                .push(json!({
                    "sender_id": body.get("sender_id").cloned().unwrap_or(serde_json::Value::Null),
                    "sender_nick": body.get("sender").cloned().unwrap_or(serde_json::Value::Null),
                    "content": body.get("message").cloned().unwrap_or(serde_json::Value::Null),
                    "ts": body.get("ts").cloned().unwrap_or(serde_json::Value::from(0u64)),
                    "signature": body.get("signature").cloned().unwrap_or(serde_json::Value::Null),
                }));
            let mut response = json!({ "status": "ok" });
            if peers.iter().filter(|peer| *peer != &state.peer_id).count() == 0 {
                response["broadcast"] = serde_json::Value::String("local_only".to_string());
            }
            AxumJson(response).into_response()
        }
        ("peer", "gossip_recv") => {
            let topic = body
                .get("topic")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let consumer_id = body
                .get("consumer_id")
                .and_then(|value| value.as_str())
                .unwrap_or("default");
            let limit = body
                .get("limit")
                .and_then(|value| value.as_u64())
                .unwrap_or(50);
            let skip_sender_id = body
                .get("skip_sender_id")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let mut bus = state.bus.lock().await;
            let cursor_key = (
                state.peer_id.clone(),
                topic.to_string(),
                consumer_id.to_string(),
            );
            let start = *bus.cursors.get(&cursor_key).unwrap_or(&0);
            let all = bus.topic_messages.get(topic).cloned().unwrap_or_default();
            let count = all.len().saturating_sub(start).min(limit as usize);
            let selected = all
                .into_iter()
                .skip(start)
                .take(limit as usize)
                .filter(|message| {
                    skip_sender_id.is_empty()
                        || message
                            .get("sender_id")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            != skip_sender_id
                })
                .collect::<Vec<_>>();
            bus.cursors.insert(cursor_key, start + count);
            AxumJson(json!({
                "status": "ok",
                "data": { "messages": selected }
            }))
            .into_response()
        }
        _ => AxumJson(json!({
            "status": "error",
            "message": format!("unsupported fake runtime operation {scheme}/{op}"),
        }))
        .into_response(),
    }
}

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

#[tokio::test]
async fn test_room_service_assets_serve() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/apps/chat-room/")
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
    assert!(text.contains("Chat Room"));

    let wasm = app
        .oneshot(
            Request::builder()
                .uri("/apps/chat-room/chat_room_ui_bg.wasm")
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
    assert!(resp
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.starts_with(&format!("{HOME_SESSION_COOKIE}="))));
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
    elastos_identity::save_nickname(dir.path(), "anders").unwrap();

    let app = gateway_router(test_state(dir.path()));
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
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
        "Open System to view this device identity and runtime state."
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
async fn test_system_updates_home_background_image() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let system_token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();
    let home_token = home_app_token(dir.path());
    let one_pixel_png = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=")
            .unwrap();

    let updated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/appearance/background-image")
                .header("x-elastos-home-token", system_token.clone())
                .header(CONTENT_TYPE, "image/png")
                .body(Body::from(one_pixel_png))
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
        background_url.starts_with("/api/apps/home/appearance/background-image?v="),
        "{background_url}"
    );

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", home_token.clone())
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
                .header("x-elastos-home-token", system_token.clone())
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

    let image = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/appearance/background-image")
                .header("x-elastos-home-token", home_token.clone())
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
    assert!(!image_body.is_empty());

    let oversized = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/appearance/background-image")
                .header("x-elastos-home-token", system_token.clone())
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
                .header("x-elastos-home-token", system_token)
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
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/appearance/background-image")
                .header("x-elastos-home-token", home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_image.status(), StatusCode::NOT_FOUND);
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
    elastos_identity::save_nickname(dir.path(), "anders").unwrap();

    let app = gateway_router(test_state(dir.path()));
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
    assert_eq!(payload["identity"]["handle"], "anders");
    assert!(payload["identity"]["device_did"].is_string());
    assert_eq!(payload["home"]["id"], "home");
    assert_eq!(payload["home"]["route"], "/apps/home/");
    assert_eq!(payload["app"]["id"], "system");
    assert_eq!(payload["app"]["route"], "/apps/system/");
    assert_eq!(payload["runtime"]["running"], false);
    assert_eq!(payload["storage"]["available"], false);
    assert_eq!(payload["storage"]["note"], "Document provider unavailable.");
    assert!(payload.get("instance").is_none());
    assert_eq!(payload["runtime_log"]["available"], false);
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

#[tokio::test]
async fn test_documents_provider_routes_require_home_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(documents_test_state(dir.path()).await);

    let denied = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/documents/summary")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_inbox_routes_require_home_token_and_return_notifications() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/inbox/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let token = issue_home_launch_token(dir.path(), INBOX_CAPSULE_ID).unwrap();
    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", token.clone())
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
    assert_eq!(summary_payload["app"]["id"], INBOX_CAPSULE_ID);
    assert_eq!(summary_payload["app"]["route"], "/apps/inbox/");
    assert_eq!(summary_payload["notifications"]["unread_count"], 0);

    let bad_action = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"action_id":"unknown-action"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_action.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_documents_provider_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(documents_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), DOCUMENTS_CAPSULE_ID).unwrap();

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/documents/create")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"Provider Notes"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let created_payload: serde_json::Value = serde_json::from_slice(&created_body).unwrap();
    assert_eq!(created_payload["status"], "ok");
    let doc_did = created_payload["data"]["document"]["doc_did"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        created_payload["data"]["document"]["document_uri"],
        format!("localhost://ElastOS/Documents/{}", doc_did)
    );

    let saved = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/documents/save")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "doc_did": doc_did,
                        "title": "Provider Notes",
                        "body": "# Provider Notes\n\nSaved through provider.\n",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/documents/summary")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_payload: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(summary_payload["status"], "ok");
    assert!(summary_payload["data"]["documents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["doc_did"] == doc_did
                && item["title"] == "Provider Notes"
                && item["document_uri"] == format!("localhost://ElastOS/Documents/{}", doc_did)
        }));

    let fetched = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/documents/get")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "doc_did": doc_did,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fetched.status(), StatusCode::OK);
    let fetched_body = axum::body::to_bytes(fetched.into_body(), usize::MAX)
        .await
        .unwrap();
    let fetched_payload: serde_json::Value = serde_json::from_slice(&fetched_body).unwrap();
    assert_eq!(fetched_payload["status"], "ok");
    assert_eq!(
        fetched_payload["data"]["document"]["body"],
        "# Provider Notes\n\nSaved through provider.\n"
    );
    assert_eq!(
        fetched_payload["data"]["document"]["document_uri"],
        format!("localhost://ElastOS/Documents/{}", doc_did)
    );

    let deleted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/documents/delete")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "doc_did": doc_did,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let deleted_body = axum::body::to_bytes(deleted.into_body(), usize::MAX)
        .await
        .unwrap();
    let deleted_payload: serde_json::Value = serde_json::from_slice(&deleted_body).unwrap();
    assert_eq!(deleted_payload["status"], "ok");

    let summary_after_delete = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/documents/summary")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary_after_delete.status(), StatusCode::OK);
    let summary_after_delete_body =
        axum::body::to_bytes(summary_after_delete.into_body(), usize::MAX)
            .await
            .unwrap();
    let summary_after_delete_payload: serde_json::Value =
        serde_json::from_slice(&summary_after_delete_body).unwrap();
    assert!(!summary_after_delete_payload["data"]["documents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["doc_did"] == doc_did));
}

#[tokio::test]
async fn test_library_home_token_can_read_documents_provider_summary() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(documents_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), LIBRARY_CAPSULE_ID).unwrap();

    let summary = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/documents/summary")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_payload: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(summary_payload["status"], "ok");
    assert!(summary_payload["data"]["documents"].is_array());
}

#[tokio::test]
async fn test_library_home_token_cannot_mutate_documents_provider() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(documents_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), LIBRARY_CAPSULE_ID).unwrap();

    let denied = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/provider/documents/create")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"Should Fail"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let denied_body = axum::body::to_bytes(denied.into_body(), usize::MAX)
        .await
        .unwrap();
    let denied_text = String::from_utf8(denied_body.to_vec()).unwrap();
    assert!(denied_text.contains("home launch token is not authorized for this provider"));
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
async fn test_system_handle_update_persists_and_chat_room_uses_handle() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "handle-peer").await;
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
    let route = payload["route"].as_str().unwrap();
    let token = route.split("home_token=").nth(1).unwrap();

    let update = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/system/identity/handle")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"handle":"anders"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::OK);
    let body = axum::body::to_bytes(update.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["handle"], "anders");

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", token)
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
    assert_eq!(payload["identity"]["handle"], "anders");

    let chat_launch = app
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
    assert_eq!(payload["display_name"], "anders");
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
async fn test_chat_room_session_start_connects_open_room_local_runtime() {
    let dir = tempfile::tempdir().unwrap();
    elastos_identity::save_nickname(dir.path(), "anders").unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "open-room-peer").await;
    let app = gateway_router(test_state(dir.path()));

    let launch = app
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
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap();
    let token = route.split("home_token=").nth(1).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
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
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["status"], "connected");
    assert_eq!(payload["display_name"], "anders");
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

    let launch = app
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
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap();
    let token = route.split("home_token=").nth(1).unwrap();

    let denied = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
                .header("x-elastos-home-token", token)
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

    let launch = app
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
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap();
    let token = route.split("home_token=").nth(1).unwrap();

    let response = app
        .oneshot(
            Request::builder()
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
    assert_eq!(payload["display_name"], "Local runtime");
}

#[tokio::test]
async fn test_chat_room_shell_requests_use_shell_launch_authority_without_room_cookie() {
    let dir = tempfile::tempdir().unwrap();
    elastos_identity::save_nickname(dir.path(), "anders").unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "shell-room-peer").await;
    let app = gateway_router(test_state(dir.path()));

    let launch = app
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
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap();
    let token = route.split("home_token=").nth(1).unwrap();

    let send = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/objects/send")
                .header("x-elastos-home-token", token)
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
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header("x-elastos-home-token", token)
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
            Request::builder()
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
            Request::builder()
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
    elastos_identity::save_nickname(dir.path(), "anders").unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "room-cookie-peer").await;
    let app = gateway_router(test_state(dir.path()));

    let launch = app
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
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let home_token = payload["route"]
        .as_str()
        .unwrap()
        .split("home_token=")
        .nth(1)
        .unwrap();

    let native_session = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
                .header("x-elastos-home-token", home_token)
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
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap();
    let home_token = route.split("home_token=").nth(1).unwrap();

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
            Request::builder()
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
async fn test_viewer_gateway_routes_list_and_serve_viewer_bound_capsules() {
    let dir = tempfile::tempdir().unwrap();
    write_test_capsule_manifest(dir.path(), "gba-emulator");
    write_test_browser_capsule(dir.path(), "other-viewer", "viewer", "Other viewer", None);
    write_test_viewer_capsule(
        dir.path(),
        "demo-rom",
        "gba-emulator",
        "demo.gba",
        "Demo ROM",
    );
    write_test_viewer_capsule(
        dir.path(),
        "other-rom",
        "other-viewer",
        "other.gba",
        "Other ROM",
    );
    let app = gateway_router(test_state(dir.path()));
    let home_token = issue_home_launch_token(dir.path(), GBA_EMULATOR_CAPSULE_ID).unwrap();

    let denied_library = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/viewers/gba-emulator/library")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied_library.status(), StatusCode::UNAUTHORIZED);

    let library = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/viewers/gba-emulator/library")
                .header("x-elastos-home-token", home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(library.status(), StatusCode::OK);
    let body = axum::body::to_bytes(library.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["items"][0]["capsule"], "demo-rom");
    assert_eq!(payload["items"][0]["entrypoint"], "demo.gba");

    let rom_home_token = issue_home_launch_token(dir.path(), "demo-rom").unwrap();
    let rom_library = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/viewers/gba-emulator/library")
                .header("x-elastos-home-token", rom_home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rom_library.status(), StatusCode::OK);

    let content = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/viewers/gba-emulator/content/demo-rom")
                .header("x-elastos-home-token", rom_home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(content.status(), StatusCode::OK);
    assert_eq!(
        content
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/octet-stream")
    );
    let body = axum::body::to_bytes(content.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"rom-data");

    let wrong_viewer_token = issue_home_launch_token(dir.path(), "other-rom").unwrap();
    let wrong_viewer = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/viewers/gba-emulator/content/other-rom")
                .header("x-elastos-home-token", wrong_viewer_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_viewer.status(), StatusCode::NOT_FOUND);

    let non_viewer = app
        .oneshot(
            Request::builder()
                .uri("/api/viewers/documents/library")
                .header("x-elastos-home-token", home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(non_viewer.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_viewer_gateway_storage_routes_require_home_token_and_round_trip_bytes() {
    let dir = tempfile::tempdir().unwrap();
    write_test_capsule_manifest(dir.path(), "gba-emulator");
    write_test_viewer_capsule(
        dir.path(),
        "demo-rom",
        "gba-emulator",
        "demo.gba",
        "Demo ROM",
    );
    let home_token = issue_home_launch_token(dir.path(), "gba-emulator").unwrap();
    let app = gateway_router(test_state(dir.path()));

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/viewers/gba-emulator/storage/demo-rom/state/demo.ss1")
                .body(Body::from("save-state"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let put = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/viewers/gba-emulator/storage/demo-rom/state/demo.ss1")
                .header("x-elastos-home-token", home_token.clone())
                .body(Body::from("save-state"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::NO_CONTENT);

    let get = app
        .oneshot(
            Request::builder()
                .uri("/api/viewers/gba-emulator/storage/demo-rom/state/demo.ss1")
                .header("x-elastos-home-token", home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::OK);
    let body = axum::body::to_bytes(get.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"save-state");
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
    let _runtime = start_fake_runtime(dir.path(), bus, "chat-room-peer").await;
    let app = gateway_router(test_state(dir.path()));

    let launch = app
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

    let summary = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", system_app_token(dir.path()))
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
async fn test_home_launch_reports_system_launch_failure_without_runtime() {
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
        .contains("local runtime is not running"));
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

    let owner_token = crate::room_service::start_local_runtime_session(
        owner_dir.path(),
        &owner_did,
        "Owner",
        "WSL",
    )
    .unwrap()
    .token;

    let guest_token = crate::room_service::start_local_runtime_session(
        guest_dir.path(),
        &guest_did,
        "Guest",
        "Jetson",
    )
    .unwrap()
    .token;

    let owner_gateway = gateway_router(test_state(owner_dir.path()));
    let guest_gateway = gateway_router(test_state(guest_dir.path()));

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

    let poll_response = guest_gateway
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(AUTHORIZATION, format!("Bearer {}", guest_token))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(poll_response.status(), StatusCode::OK);
    let poll_body = axum::body::to_bytes(poll_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let poll: serde_json::Value = serde_json::from_slice(&poll_body).unwrap();
    assert_eq!(poll["transport"]["connected_peer_count"].as_u64(), Some(1));
    assert!(poll["transport"]["status"]
        .as_str()
        .unwrap_or_default()
        .contains("Carrier room sync connected to 1 runtime"));
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

    let owner_token = crate::room_service::start_local_runtime_session(
        owner_dir.path(),
        &owner_did,
        "Owner",
        "WSL",
    )
    .unwrap()
    .token;

    let guest_token = crate::room_service::start_local_runtime_session(
        guest_dir.path(),
        &guest_did,
        "Guest",
        "Jetson",
    )
    .unwrap()
    .token;

    let owner_gateway = gateway_router(test_state(owner_dir.path()));
    let guest_gateway = gateway_router(test_state(guest_dir.path()));

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

    let poll_response = guest_gateway
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(AUTHORIZATION, format!("Bearer {}", guest_token))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(poll_response.status(), StatusCode::OK);
    let poll_body = axum::body::to_bytes(poll_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let poll: serde_json::Value = serde_json::from_slice(&poll_body).unwrap();
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

    let owner_token = crate::room_service::start_local_runtime_session(
        owner_dir.path(),
        &owner_did,
        "Owner",
        "WSL",
    )
    .unwrap()
    .token;

    let guest_token = crate::room_service::start_local_runtime_session(
        guest_dir.path(),
        &guest_did,
        "Guest",
        "Jetson",
    )
    .unwrap()
    .token;

    let owner_gateway = gateway_router(test_state(owner_dir.path()));
    let guest_gateway = gateway_router(test_state(guest_dir.path()));

    let owner_first_poll = owner_gateway
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(owner_first_poll.status(), StatusCode::OK);

    let guest_first_poll = guest_gateway
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(AUTHORIZATION, format!("Bearer {}", guest_token))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_first_poll.status(), StatusCode::OK);
    let guest_body = axum::body::to_bytes(guest_first_poll.into_body(), usize::MAX)
        .await
        .unwrap();
    let guest_poll: serde_json::Value = serde_json::from_slice(&guest_body).unwrap();
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

    let owner_second_poll = owner_gateway
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(owner_second_poll.status(), StatusCode::OK);
    let owner_body = axum::body::to_bytes(owner_second_poll.into_body(), usize::MAX)
        .await
        .unwrap();
    let owner_poll: serde_json::Value = serde_json::from_slice(&owner_body).unwrap();
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

    let owner_after_leave = owner_gateway
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/poll")
                .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"since":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(owner_after_leave.status(), StatusCode::OK);
    let owner_after_leave_body = axum::body::to_bytes(owner_after_leave.into_body(), usize::MAX)
        .await
        .unwrap();
    let owner_after_leave_json: serde_json::Value =
        serde_json::from_slice(&owner_after_leave_body).unwrap();
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
