use super::*;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn error_code(response: Response) -> String {
    serde_json::to_value(response).unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string()
}

fn cleanup_binding_for(provider: &BrowserEngineAdapter, page_id: &str) -> EngineCleanupBinding {
    engine_cleanup_binding(
        page_id,
        provider
            .page_control_sessions
            .get(page_id)
            .expect("test page control session"),
    )
}

fn typed_supervisor_cleanup_receipt(request: &str) -> String {
    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .expect("test cleanup request body");
    let request: serde_json::Value =
        serde_json::from_str(body).expect("typed cleanup request JSON");
    let binding = request
        .get("runtime_cleanup")
        .cloned()
        .expect("Runtime cleanup binding");
    serde_json::to_string(&json!({
        "schema": BROWSER_SUPERVISOR_CLEANUP_RESULT_SCHEMA,
        "page_id": binding["page_id"],
        "generation": binding["generation"],
        "binding": binding,
        "terminal": true,
        "effects": {
            "page_absent": true,
            "child_absent": true,
            "vm_absent": true,
            "route_absent": true,
            "socket_absent": true
        }
    }))
    .unwrap()
}

fn http_request_is_complete(request: &[u8]) -> bool {
    let text = String::from_utf8_lossy(request);
    let Some((headers, body)) = text.split_once("\r\n\r\n") else {
        return false;
    };
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
        })
        .unwrap_or(0);
    body.len() >= content_length
}

fn stream_receipt(byte_transport: &str) -> StreamSessionReceipt {
    StreamSessionReceipt {
        schema: "elastos.exit.stream-session/v1".to_string(),
        stream_id: "stream:proof:test".to_string(),
        target: "tls://glidefinance.io:443".to_string(),
        byte_transport: byte_transport.to_string(),
        adapter_ipc: (byte_transport == "adapter_ipc").then(|| AdapterIpcEndpoint {
            schema: "elastos.adapter-ipc/v1".to_string(),
            kind: AdapterIpcKind::UnixSocket,
            path: "/tmp/elastos-browser-stream.sock".to_string(),
            stream_id: "stream:proof:test".to_string(),
            runtime_stream_path: Some("/tmp/elastos-runtime-stream.sock".to_string()),
        }),
        relay_ipc: None,
    }
}

fn test_browser_profile() -> BrowserProfileDescriptor {
    BrowserProfileDescriptor {
        schema: "elastos.browser.profile/v1".to_string(),
        scope: "active_principal".to_string(),
        storage: "principal_owned_profile_disk".to_string(),
        storage_posture: "principal_owned_reset_scoped_unprotected".to_string(),
        protected_storage: false,
        encrypted: false,
        recoverable: false,
        recovery: "not_recovery_kit_packaged".to_string(),
        uri: "localhost://Users/0123456789ab/BrowserProfiles/default/profile.ext4".to_string(),
        public_uri: "localhost://Users/self/BrowserProfiles/default/profile.ext4".to_string(),
        profile_key: "profile-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            .to_string(),
        disk_path: "/tmp/elastos-browser-profile-test/BrowserProfiles/default/profile.ext4"
            .to_string(),
        reset: "whole_profile".to_string(),
    }
}

fn spawn_status_socket(body: Value) -> String {
    spawn_status_socket_requests(body, 1)
}

fn spawn_status_socket_requests(body: Value, request_count: usize) -> String {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = format!(
        "/tmp/elastos-browser-engine-adapter-test-{}-{suffix}.sock",
        std::process::id()
    );
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("test status socket should bind");
    listener
        .set_nonblocking(true)
        .expect("test status socket should be nonblocking");
    let response_body = body.to_string();
    let socket_path = path.clone();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut handled = 0_usize;
        while handled < request_count && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = [0_u8; 1024];
                    let _ = stream.read(&mut buffer);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        response_body.len(),
                        response_body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    handled += 1;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        let _ = std::fs::remove_file(socket_path);
    });
    path
}

fn spawn_unresponsive_status_socket() -> String {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = format!(
        "/tmp/elastos-browser-engine-adapter-unresponsive-{}-{suffix}.sock",
        std::process::id()
    );
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("unresponsive test socket should bind");
    let socket_path = path.clone();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0_u8; 2048];
            let _ = stream.read(&mut buffer);
            std::thread::sleep(Duration::from_secs(3));
        }
        let _ = std::fs::remove_file(socket_path);
    });
    path
}

fn proof_adapter_config() -> Value {
    json!({
        "adapters": [{
            "id": "linux-proof",
            "kind": "contract_proof",
            "display_modes": ["webrtc_remote_display"]
        }]
    })
}

#[test]
fn status_is_unavailable_without_configured_adapter() {
    let mut provider = BrowserEngineAdapter::new();
    let response =
        serde_json::to_value(provider.status(Some("person:local:test".to_string()))).unwrap();
    assert_eq!(response["status"], "ok");
    assert_eq!(
        response["data"]["protocol_version"],
        BROWSER_ENGINE_PROTOCOL_VERSION
    );
    assert_eq!(response["data"]["status"], "unavailable");
    assert_eq!(response["data"]["direct_network"], false);
    assert_eq!(response["data"]["wallet_injection"], false);
}

#[test]
fn protocol_v2_rejects_old_launch_before_dispatching_any_effect() {
    let provider = BrowserEngineAdapter::new();
    let old_launch = json!({
        "op": "launch",
        "url": "https://glidefinance.io/",
        "stream_session": {
            "schema": "elastos.exit.stream-session/v1",
            "stream_id": "stream:proof:test",
            "target": "tls://glidefinance.io:443",
            "byte_transport": "adapter_ipc",
            "adapter_ipc": {
                "schema": "elastos.adapter-ipc/v1",
                "kind": "unix_socket",
                "path": "/tmp/elastos-browser-stream.sock",
                "stream_id": "stream:proof:test",
                "runtime_stream_path": "/tmp/elastos-runtime-stream.sock"
            }
        },
        "profile": test_browser_profile(),
        "display_mode": "webrtc_remote_display",
        "guarantee_level": "operator_rbi"
    });

    let error = decode_request(&old_launch.to_string()).unwrap_err();

    assert!(error.to_string().contains("lifecycle_generation"));
    assert!(provider.page_control_sessions.is_empty());
}

#[test]
fn provider_bridge_default_config_initializes_empty() {
    let mut provider = BrowserEngineAdapter::new();
    let response = serde_json::to_value(provider.init(json!({
        "base_path": "",
        "allowed_paths": [],
        "read_only": false,
        "encryption_key": ""
    })))
    .unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["adapter_count"], 0);
}

#[test]
fn configured_adapter_reports_contract_without_raw_authority() {
    let mut provider = BrowserEngineAdapter::new();
    let response = serde_json::to_value(provider.init(proof_adapter_config())).unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["adapter_count"], 1);

    let status = serde_json::to_value(provider.status(None)).unwrap();
    assert_eq!(status["data"]["status"], "configured");
    assert_eq!(status["data"]["required_byte_transport"], "adapter_ipc");
    assert_eq!(
        status["data"]["display_session_schema"],
        "elastos.browser.display-session/v1"
    );
    assert_eq!(
        status["data"]["supported_display_modes"][0],
        "webrtc_remote_display"
    );
    assert_eq!(
        status["data"]["supported_guarantee_levels"][0],
        "operator_rbi"
    );
}

#[test]
fn launch_fails_closed_without_configured_adapter() {
    let mut provider = BrowserEngineAdapter::new();
    assert_eq!(
        error_code(provider.launch(
            "https://glidefinance.io/",
            &stream_receipt("adapter_ipc"),
            None,
            None,
            json!({})
        )),
        "engine_unavailable"
    );
}

#[test]
fn launch_requires_attached_byte_transport() {
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(proof_adapter_config());
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    assert_eq!(
        error_code(provider.launch(
            "https://glidefinance.io/",
            &stream_receipt("not_attached"),
            None,
            None,
            json!({})
        )),
        "byte_transport_unavailable"
    );
}

#[test]
fn launch_rejects_invalid_browser_profile_descriptor() {
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(proof_adapter_config());
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let mut profile = test_browser_profile();
    profile.storage = "runtime_owned_profile_disk".to_string();

    assert_eq!(
        error_code(provider.launch_with_viewport(LaunchContext {
            url: "https://glidefinance.io/",
            stream_session: &stream_receipt("adapter_ipc"),
            profile,
            adapter_id: None,
            principal_id: Some("person:local:test".to_string()),
            reason: Some("open browser page".to_string()),
            wallet: json!({}),
            viewport: None,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
        })),
        "invalid_profile"
    );

    let mut profile = test_browser_profile();
    profile.encrypted = true;

    assert_eq!(
        error_code(provider.launch_with_viewport(LaunchContext {
            url: "https://glidefinance.io/",
            stream_session: &stream_receipt("adapter_ipc"),
            profile,
            adapter_id: None,
            principal_id: Some("person:local:test".to_string()),
            reason: Some("open browser page".to_string()),
            wallet: json!({}),
            viewport: None,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
        })),
        "invalid_profile"
    );
}

#[test]
fn launch_rejects_unimplemented_product_display_modes() {
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(proof_adapter_config());
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    assert_eq!(
        error_code(provider.launch_with_viewport(LaunchContext {
            url: "https://glidefinance.io/",
            stream_session: &stream_receipt("adapter_ipc"),
            profile: test_browser_profile(),
            adapter_id: None,
            principal_id: None,
            reason: None,
            wallet: json!({}),
            viewport: None,
            display_mode: BrowserDisplayMode::NativeSurface,
            guarantee_level: BrowserGuaranteeLevel::PolicyWebview,
        })),
        "display_session_unavailable"
    );
}

#[test]
fn launch_rejects_mismatched_guarantee_level() {
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(proof_adapter_config());
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    assert_eq!(
        error_code(provider.launch_with_viewport(LaunchContext {
            url: "https://glidefinance.io/",
            stream_session: &stream_receipt("adapter_ipc"),
            profile: test_browser_profile(),
            adapter_id: None,
            principal_id: None,
            reason: None,
            wallet: json!({}),
            viewport: None,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::PolicyWebview,
        })),
        "guarantee_unavailable"
    );
}

#[test]
fn launch_rejects_adapter_ipc_without_descriptor() {
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(proof_adapter_config());
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let mut receipt = stream_receipt("adapter_ipc");
    receipt.adapter_ipc = None;

    assert_eq!(
        error_code(provider.launch("https://glidefinance.io/", &receipt, None, None, json!({}))),
        "invalid_stream_session"
    );
}

#[test]
fn launch_rejects_mismatched_adapter_ipc_descriptor() {
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(proof_adapter_config());
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let mut receipt = stream_receipt("adapter_ipc");
    receipt.adapter_ipc.as_mut().unwrap().stream_id = "stream:other".to_string();

    assert_eq!(
        error_code(provider.launch("https://glidefinance.io/", &receipt, None, None, json!({}))),
        "invalid_stream_session"
    );
}

#[test]
fn launch_rejects_invalid_runtime_stream_path() {
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(proof_adapter_config());
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let mut receipt = stream_receipt("adapter_ipc");
    receipt.adapter_ipc.as_mut().unwrap().runtime_stream_path =
        Some("tcp://127.0.0.1:9999".to_string());

    assert_eq!(
        error_code(provider.launch("https://glidefinance.io/", &receipt, None, None, json!({}))),
        "invalid_stream_session"
    );
}

#[test]
fn launch_accepts_attached_adapter_ipc_stream_receipt() {
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(proof_adapter_config());
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let response = serde_json::to_value(provider.launch(
        "https://glidefinance.io/",
        &stream_receipt("adapter_ipc"),
        Some("person:local:test".to_string()),
        Some("open browser page".to_string()),
        json!({}),
    ))
    .unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["schema"], "elastos.browser.engine.page/v1");
    assert_eq!(response["data"]["adapter"], "linux-proof");
    assert_eq!(response["data"]["direct_network"], false);
    assert_eq!(response["data"]["wallet_injection"], false);
    assert_eq!(
        response["data"]["display_session"]["schema"],
        "elastos.browser.display-session/v1"
    );
    assert_eq!(
        response["data"]["display_session"]["mode"],
        "webrtc_remote_display"
    );
}

#[test]
fn launch_can_select_configured_adapter_id() {
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(json!({
        "adapters": [
            {
                "id": "mac-engine",
                "kind": "contract_proof",
                "display_modes": ["webrtc_remote_display"]
            },
            {
                "id": "jetson-engine",
                "kind": "contract_proof",
                "display_modes": ["webrtc_remote_display"]
            }
        ]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let status = serde_json::to_value(provider.status(None)).unwrap();
    assert_eq!(status["data"]["adapter_count"], 2);
    assert_eq!(status["data"]["adapters"][0]["id"], "mac-engine");
    assert_eq!(status["data"]["adapters"][0]["default"], true);
    assert_eq!(
        status["data"]["adapters"][0]["backing_substrate"],
        "operator_rbi"
    );
    assert_eq!(status["data"]["adapters"][1]["id"], "jetson-engine");
    assert_eq!(
        status["data"]["adapters"][1]["supported_display_modes"][0],
        "webrtc_remote_display"
    );

    let response = serde_json::to_value(provider.launch_with_viewport(LaunchContext {
        url: "https://ela.city/",
        stream_session: &stream_receipt("adapter_ipc"),
        profile: test_browser_profile(),
        adapter_id: Some("jetson-engine".to_string()),
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
        guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
    }))
    .unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["adapter"], "jetson-engine");
}

#[test]
fn status_reports_remote_vz_backing_substrate_without_host_paths() {
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(json!({
        "adapters": [{
            "id": "browser-vm-product",
            "kind": "chromium_microvm",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/tmp/browser-vm-remote-vz-launcher",
                "control_socket_path": "/tmp/elastos-browser-vm-control-test.sock"
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let status = serde_json::to_value(provider.status(None)).unwrap();
    assert_eq!(
        status["data"]["adapters"][0]["backing_substrate"],
        "remote_operator_vm"
    );
    let public_status = status.to_string();
    assert!(!public_status.contains("browser-vm-remote-vz-launcher"));
    assert!(!public_status.contains("elastos-browser-vm-control-test.sock"));
}

#[test]
fn launch_rejects_unknown_or_unsafe_adapter_id() {
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(proof_adapter_config());
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    assert_eq!(
        error_code(provider.launch_with_viewport(LaunchContext {
            url: "https://ela.city/",
            stream_session: &stream_receipt("adapter_ipc"),
            profile: test_browser_profile(),
            adapter_id: Some("missing-engine".to_string()),
            principal_id: Some("person:local:test".to_string()),
            reason: Some("open browser page".to_string()),
            wallet: json!({}),
            viewport: None,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
        })),
        "engine_unavailable"
    );
    assert_eq!(
        error_code(provider.launch_with_viewport(LaunchContext {
            url: "https://ela.city/",
            stream_session: &stream_receipt("adapter_ipc"),
            profile: test_browser_profile(),
            adapter_id: Some("../mac-engine".to_string()),
            principal_id: Some("person:local:test".to_string()),
            reason: Some("open browser page".to_string()),
            wallet: json!({}),
            viewport: None,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
        })),
        "invalid_request"
    );
}

#[test]
fn init_rejects_native_adapter_without_supervisor() {
    let mut provider = BrowserEngineAdapter::new();
    assert_eq!(
        error_code(provider.init(json!({
            "adapters": [{
                "id": "linux-chromium-headless",
                "kind": "chromium_headless"
            }]
        }))),
        "invalid_config"
    );
}

#[test]
fn init_accepts_hosted_product_supervisor_timeout_for_heavy_launches() {
    let mut provider = BrowserEngineAdapter::new();
    let response = serde_json::to_value(provider.init(json!({
        "adapters": [{
            "id": "hosted-product",
            "kind": "selkies_gstreamer",
            "network_mode": "runtime_net_only",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/true",
                "timeout_ms": 300000,
                "control_socket_path": "/tmp/elastos-browser-test.sock"
            }
        }]
    })))
    .unwrap();
    assert_eq!(response["status"], "ok");
}

#[test]
fn init_rejects_supervisor_timeout_above_hosted_limit() {
    let mut provider = BrowserEngineAdapter::new();
    assert_eq!(
        error_code(provider.init(json!({
            "adapters": [{
                "id": "hosted-product",
                "kind": "selkies_gstreamer",
                "network_mode": "runtime_net_only",
                "display_modes": ["webrtc_remote_display"],
                "supervisor": {
                    "program": "/bin/true",
                    "timeout_ms": 300001,
                    "control_socket_path": "/tmp/elastos-browser-test.sock"
                }
            }]
        }))),
        "invalid_config"
    );
}

#[test]
fn native_adapter_launches_only_through_supervisor_result_contract() {
    let mut provider = BrowserEngineAdapter::new();
    let script = r#"printf '%s' "$ELASTOS_BROWSER_ENGINE_REQUEST" | grep -q '"principal_id":"person:local:test"' || exit 7; printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:native-proof","adapter":"linux-chromium-headless","engine":"chromium_headless","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"webrtc_remote_display","network_mode":"runtime_net_only","direct_network":false,"input":"runtime_route","audio":false,"video":true,"signaling_url":"/api/apps/browser/pages/page%3Anative-proof/webrtc"}}'"#;
    let init = provider.init(json!({
        "adapters": [{
            "id": "linux-chromium-headless",
            "kind": "chromium_headless",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/sh",
                "args": ["-c", script],
                "timeout_ms": 2000
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let response = serde_json::to_value(provider.launch(
        "https://glidefinance.io/",
        &stream_receipt("adapter_ipc"),
        Some("person:local:test".to_string()),
        Some("open browser page".to_string()),
        json!({}),
    ))
    .unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["page_id"], "page:native-proof");
    assert_eq!(response["data"]["engine"], "chromium_headless");
    assert_eq!(response["data"]["rendering"], "host_supervisor");
    assert_eq!(response["data"]["direct_network"], false);
    assert_eq!(response["data"]["wallet_injection"], false);
}

#[test]
fn supervisor_launch_clears_configured_prewarm_flag() {
    let mut provider = BrowserEngineAdapter::new();
    let script = r#"if [ "$ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE" = "1" ] && [ -z "$ELASTOS_BROWSER_ENGINE_REQUEST" ]; then printf '%s\n' '{"schema":"elastos.browser.vm-engine-prewarm/v1","ok":true,"control_socket_path":"/tmp/elastos-browser-vm-prewarm.sock","control_status":{"schema":"elastos.browser.vm-control-service.status/v1","pid":123,"active_pages":0,"max_active_pages":2,"network_mode":"runtime_net_only","direct_network":false},"network_mode":"runtime_net_only","direct_network":false}'; exit 0; fi; if [ "$ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE" = "1" ]; then echo prewarm leaked into launch >&2; exit 9; fi; printf '%s' "$ELASTOS_BROWSER_ENGINE_REQUEST" | grep -q '"stream_id":"stream:proof:test"' || exit 8; printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:prewarm-clear-proof","adapter":"linux-chromium-headless","engine":"chromium_headless","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"webrtc_remote_display","network_mode":"runtime_net_only","direct_network":false,"input":"runtime_route","audio":false,"video":true,"signaling_url":"/api/apps/browser/pages/page%3Aprewarm-clear-proof/webrtc"}}'"#;
    let init = provider.init(json!({
        "adapters": [{
            "id": "linux-chromium-headless",
            "kind": "chromium_headless",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/sh",
                "args": ["-c", script],
                "timeout_ms": 2000,
                "control_socket_path": "/tmp/elastos-browser-vm-prewarm.sock",
                "env": {
                    "ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE": "1"
                }
            }
        }]
    }));
    let init = serde_json::to_value(init).unwrap();
    assert_eq!(init["status"], "ok");
    assert_eq!(init["data"]["prewarm_results"][0]["status"], "ok");

    let response = serde_json::to_value(provider.launch(
        "https://glidefinance.io/",
        &stream_receipt("adapter_ipc"),
        Some("person:local:test".to_string()),
        Some("open browser page".to_string()),
        json!({}),
    ))
    .unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["page_id"], "page:prewarm-clear-proof");
}

#[test]
fn supervisor_launch_preserves_typed_resources_in_use_failure() {
    let mut provider = BrowserEngineAdapter::new();
    let script = r#"printf '%s\n' '{"schema":"elastos.browser.engine.launch-error/v1","code":"resources_in_use","message":"Browser profile disk is already attached to an active VM"}' >&2; exit 23"#;
    let init = provider.init(json!({
        "adapters": [{
            "id": "mac-vm-product",
            "kind": "chromium_microvm",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/sh",
                "args": ["-c", script],
                "timeout_ms": 2000
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let stream = stream_receipt("adapter_ipc");
    let response = serde_json::to_value(provider.launch_with_viewport(LaunchContext {
        url: "https://glidefinance.io/",
        stream_session: &stream,
        profile: test_browser_profile(),
        adapter_id: Some("mac-vm-product".to_string()),
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
        guarantee_level: BrowserGuaranteeLevel::MechanismMicrovm,
    }))
    .unwrap();

    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "resources_in_use");
    assert_eq!(
        response["message"],
        "Browser profile disk is already attached to an active VM"
    );
}

#[test]
fn native_adapter_can_declare_webrtc_display_mode() {
    let mut provider = BrowserEngineAdapter::new();
    let script = r#"printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:webrtc-proof","adapter":"linux-chromium-headless","engine":"chromium_headless","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"webrtc_remote_display","network_mode":"runtime_net_only","direct_network":false,"input":"runtime_route","audio":false,"video":true,"signaling_url":"/api/apps/browser/pages/page%3Awebrtc-proof/webrtc"}}'"#;
    let init = provider.init(json!({
        "adapters": [{
            "id": "linux-chromium-headless",
            "kind": "chromium_headless",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/sh",
                "args": ["-c", script],
                "timeout_ms": 2000
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");
    let status = serde_json::to_value(provider.status(None)).unwrap();
    assert_eq!(
        status["data"]["supported_display_modes"][0],
        "webrtc_remote_display"
    );

    let response = serde_json::to_value(provider.launch_with_viewport(LaunchContext {
        url: "https://glidefinance.io/",
        stream_session: &stream_receipt("adapter_ipc"),
        profile: test_browser_profile(),
        adapter_id: None,
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
        guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
    }))
    .unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["page_id"], "page:webrtc-proof");
    assert_eq!(
        response["data"]["display_session"]["mode"],
        "webrtc_remote_display"
    );
    assert_eq!(
        response["data"]["display_session"]["signaling_url"],
        "/api/apps/browser/pages/page%3Awebrtc-proof/webrtc"
    );
}

#[test]
fn webrtc_proof_surface_cannot_advertise_audio() {
    let mut provider = BrowserEngineAdapter::new();
    let script = r#"printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:webrtc-proof-audio","adapter":"linux-chromium-headless","engine":"chromium_headless","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"webrtc_remote_display","network_mode":"runtime_net_only","direct_network":false,"input":"datachannel","width":1280,"height":720,"display_backend":"cdp_screencast_i420","backend_class":"proof_surface","audio":true,"video":true,"signaling_url":"/api/apps/browser/pages/page%3Awebrtc-proof-audio/webrtc"}}'"#;
    let init = provider.init(json!({
        "adapters": [{
            "id": "linux-chromium-headless",
            "kind": "chromium_headless",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/sh",
                "args": ["-c", script],
                "timeout_ms": 2000
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    assert_eq!(
        error_code(provider.launch_with_viewport(LaunchContext {
            url: "https://youtube.com/",
            stream_session: &stream_receipt("adapter_ipc"),
            profile: test_browser_profile(),
            adapter_id: None,
            principal_id: Some("person:local:test".to_string()),
            reason: Some("open browser page".to_string()),
            wallet: json!({}),
            viewport: None,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
        })),
        "invalid_supervisor_result"
    );
}

#[test]
fn webrtc_product_compositor_can_advertise_audio() {
    let mut provider = BrowserEngineAdapter::new();
    let script = r#"printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:webrtc-product-audio","adapter":"hosted-product","engine":"selkies_gstreamer","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"webrtc_remote_display","network_mode":"runtime_net_only","direct_network":false,"input":"datachannel","width":1280,"height":720,"display_backend":"selkies_gstreamer_webrtc","backend_class":"product_compositor","audio":true,"video":true,"signaling_url":"/api/apps/browser/pages/page%3Awebrtc-product-audio/webrtc"}}'"#;
    let init = provider.init(json!({
        "adapters": [{
            "id": "hosted-product",
            "kind": "selkies_gstreamer",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/sh",
                "args": ["-c", script],
                "timeout_ms": 2000
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let response = serde_json::to_value(provider.launch_with_viewport(LaunchContext {
        url: "https://youtube.com/",
        stream_session: &stream_receipt("adapter_ipc"),
        profile: test_browser_profile(),
        adapter_id: None,
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
        guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
    }))
    .unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["page_id"], "page:webrtc-product-audio");
    assert_eq!(response["data"]["display_session"]["audio"], true);
    assert_eq!(
        response["data"]["display_session"]["backend_class"],
        "product_compositor"
    );
    assert_eq!(
        response["data"]["display_session"]["display_backend"],
        "selkies_gstreamer_webrtc"
    );
}

#[test]
fn supervisor_launch_registers_page_scoped_control_session() {
    let mut provider = BrowserEngineAdapter::new();
    let script = r#"printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:isolated-product","adapter":"hosted-product","engine":"selkies_gstreamer","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"control_socket_path":"/tmp/elastos-browser-isolated-product.sock","isolated_session":true,"isolation":{"schema":"elastos.browser.engine.isolation/v1","kind":"per_launch_selkies_target","session_dir":"/tmp/elastos-browser-sessions/test"},"process":{"pid":42,"stream_bridge_pid":null},"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"webrtc_remote_display","network_mode":"runtime_net_only","direct_network":false,"input":"datachannel","width":1280,"height":720,"display_backend":"selkies_gstreamer_webrtc","backend_class":"product_compositor","audio":true,"video":true,"signaling_url":"/api/apps/browser/pages/page%3Aisolated-product/webrtc"}}'"#;
    let init = provider.init(json!({
        "adapters": [{
            "id": "hosted-product",
            "kind": "selkies_gstreamer",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/sh",
                "args": ["-c", script],
                "timeout_ms": 2000
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let response = serde_json::to_value(provider.launch_with_viewport(LaunchContext {
        url: "https://ela.city/",
        stream_session: &stream_receipt("adapter_ipc"),
        profile: test_browser_profile(),
        adapter_id: None,
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
        guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
    }))
    .unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["engine_control"], "page_scoped");
    assert_eq!(response["data"]["isolated_engine_session"], true);
    assert_eq!(
        response["data"]["isolation"]["kind"],
        "per_launch_selkies_target"
    );
    assert!(response["data"].get("control_socket_path").is_none());

    let session = provider
        .page_control_sessions
        .get("page:isolated-product")
        .expect("isolated page control session should be registered");
    assert_eq!(
        session.socket_path,
        "/tmp/elastos-browser-isolated-product.sock"
    );
    assert!(session.isolated_session);
}

#[test]
fn launch_fails_closed_when_session_capacity_is_full() {
    let mut provider = BrowserEngineAdapter::new();
    let status_socket = spawn_status_socket_requests(
        json!({
            "schema": "elastos.browser.vm-control-service.status/v1",
            "ok": true,
            "active_pages": 1,
            "page_ids": ["page:capacity-a"],
            "network_mode": "runtime_net_only",
            "direct_network": false
        }),
        2,
    );
    let launch_result = json!({
        "schema": "elastos.browser.engine.supervisor-result/v1",
        "page_id": "page:capacity-a",
        "adapter": "hosted-product",
        "engine": "selkies_gstreamer",
        "stream_id": "stream:proof:test",
        "network_mode": "runtime_net_only",
        "direct_network": false,
        "wallet_injection": false,
        "control_socket_path": status_socket,
        "isolated_session": true,
        "isolation": {
            "schema": "elastos.browser.engine.isolation/v1",
            "kind": "per_launch_selkies_target",
            "session_dir": "/tmp/elastos-browser-sessions/capacity-a"
        },
        "process": {"pid": 42, "stream_bridge_pid": null},
        "display_session": {
            "schema": "elastos.browser.display-session/v1",
            "session_id": "display:stream:proof:test",
            "mode": "webrtc_remote_display",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "input": "datachannel",
            "width": 1280,
            "height": 720,
            "display_backend": "selkies_gstreamer_webrtc",
            "backend_class": "product_compositor",
            "audio": true,
            "video": true,
            "signaling_url": "/api/apps/browser/pages/page%3Acapacity-a/webrtc"
        }
    });
    let script = format!("printf '%s\n' '{}'", launch_result);
    let init = provider.init(json!({
        "max_active_sessions": 1,
        "adapters": [{
            "id": "hosted-product",
            "kind": "selkies_gstreamer",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/sh",
                "args": ["-c", script],
                "timeout_ms": 2000
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let first = serde_json::to_value(provider.launch_with_viewport(LaunchContext {
        url: "https://ela.city/",
        stream_session: &stream_receipt("adapter_ipc"),
        profile: test_browser_profile(),
        adapter_id: None,
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
        guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
    }))
    .unwrap();
    assert_eq!(first["status"], "ok");

    let status = serde_json::to_value(provider.status(None)).unwrap();
    assert_eq!(status["data"]["active_sessions"], 1);
    assert_eq!(status["data"]["max_active_sessions"], 1);
    assert_eq!(status["data"]["capacity_available"], false);

    assert_eq!(
        error_code(provider.launch_with_viewport(LaunchContext {
            url: "https://glidefinance.io/",
            stream_session: &stream_receipt("adapter_ipc"),
            profile: test_browser_profile(),
            adapter_id: None,
            principal_id: Some("person:local:test".to_string()),
            reason: Some("open second browser page".to_string()),
            wallet: json!({}),
            viewport: None,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
        })),
        "browser_capacity_unavailable"
    );
}

#[test]
fn init_prewarms_configured_vm_control_service() {
    let mut provider = BrowserEngineAdapter::new();
    let script = r#"if [ "$ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE" != "1" ]; then exit 9; fi; printf '%s\n' '{"schema":"elastos.browser.vm-engine-prewarm/v1","ok":true,"control_socket_path":"/tmp/elastos-browser-vm-prewarm.sock","control_status":{"schema":"elastos.browser.vm-control-service.status/v1","pid":123,"active_pages":0,"max_active_pages":2,"network_mode":"runtime_net_only","direct_network":false},"network_mode":"runtime_net_only","direct_network":false}'"#;

    let init = serde_json::to_value(provider.init(json!({
        "max_active_sessions": 2,
        "adapters": [{
            "id": "mac-vm-product",
            "kind": "chromium_microvm",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/sh",
                "args": ["-c", script],
                "timeout_ms": 2000,
                "control_socket_path": "/tmp/elastos-browser-vm-prewarm.sock",
                "env": {
                    "ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE": "1"
                }
            }
        }]
    })))
    .unwrap();

    assert_eq!(init["status"], "ok");
    assert_eq!(
        init["data"]["prewarm_results"][0]["adapter"],
        "mac-vm-product"
    );
    assert_eq!(init["data"]["prewarm_results"][0]["status"], "ok");
    assert_eq!(
        init["data"]["prewarm_results"][0]["result"]["control_status"]["max_active_pages"],
        2
    );
}

#[test]
fn init_fails_when_configured_vm_prewarm_fails() {
    let mut provider = BrowserEngineAdapter::new();

    assert_eq!(
        error_code(provider.init(json!({
            "adapters": [{
                "id": "mac-vm-product",
                "kind": "chromium_microvm",
                "display_modes": ["webrtc_remote_display"],
                "supervisor": {
                    "program": "/bin/sh",
                    "args": ["-c", "echo prewarm failed >&2; exit 42"],
                    "timeout_ms": 2000,
                    "control_socket_path": "/tmp/elastos-browser-vm-prewarm-fail.sock",
                    "env": {
                        "ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE": "1"
                    }
                }
            }]
        }))),
        "engine_process_unavailable"
    );
}

#[test]
fn launch_reaps_stale_isolated_vm_session_before_capacity_check() {
    let mut provider = BrowserEngineAdapter::new();
    let status_socket = spawn_status_socket(json!({
        "schema": "elastos.browser.vm-control-service.status/v1",
        "ok": true,
        "active_pages": 0,
        "page_ids": [],
        "network_mode": "runtime_net_only",
        "direct_network": false
    }));
    let launch_result = json!({
        "schema": "elastos.browser.engine.supervisor-result/v1",
        "page_id": "page:capacity-reaped",
        "adapter": "mac-vm-product",
        "engine": "chromium_microvm",
        "stream_id": "stream:proof:test",
        "network_mode": "runtime_net_only",
        "direct_network": false,
        "wallet_injection": false,
        "control_socket_path": status_socket,
        "isolated_session": true,
        "isolation": {
            "schema": "elastos.browser.engine.isolation/v1",
            "kind": "per_launch_vm_target",
            "session_dir": "/tmp/elastos-browser-vm-sessions/capacity-reaped"
        },
        "process": {"pid": 42, "stream_bridge_pid": null},
        "display_session": {
            "schema": "elastos.browser.display-session/v1",
            "session_id": "display:stream:proof:test",
            "mode": "webrtc_remote_display",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "input": "datachannel",
            "width": 1280,
            "height": 720,
            "display_backend": "chromium_microvm_webrtc",
            "backend_class": "product_compositor",
            "media_transport": "runtime_relay",
            "audio": true,
            "video": true,
            "signaling_url": "/api/apps/browser/pages/page%3Acapacity-reaped/webrtc"
        }
    });
    let control_socket_path = launch_result["control_socket_path"]
        .as_str()
        .unwrap()
        .to_string();
    let script = format!("printf '%s\n' '{}'", launch_result);
    let init = provider.init(json!({
        "max_active_sessions": 1,
        "adapters": [{
            "id": "mac-vm-product",
            "kind": "chromium_microvm",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/sh",
                "args": ["-c", script],
                "timeout_ms": 2000,
                "control_socket_path": control_socket_path.clone()
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");
    provider.page_control_sessions.insert(
        "page:stale-vm".to_string(),
        PageControlSession {
            generation: "sha256:test-generation".to_string(),
            stream_id: "stream:proof:test".to_string(),
            socket_path: control_socket_path.clone(),
            shutdown_socket_path: Some(control_socket_path),
            adapter_id: "mac-vm-product".to_string(),
            principal_id: Some("person:local:test".to_string()),
            engine: AdapterKind::ChromiumMicrovm,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::MechanismMicrovm,
            isolated_session: true,
            isolation_session_dir: Some("/tmp/elastos-browser-vm-sessions/stale-vm".to_string()),
            isolation_kind: Some("per_launch_vm_target".to_string()),
            process: None,
        },
    );

    let response = serde_json::to_value(provider.launch_with_viewport(LaunchContext {
        url: "https://ela.city/",
        stream_session: &stream_receipt("adapter_ipc"),
        profile: test_browser_profile(),
        adapter_id: None,
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
        guarantee_level: BrowserGuaranteeLevel::MechanismMicrovm,
    }))
    .unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["page_id"], "page:capacity-reaped");
    assert!(!provider.page_control_sessions.contains_key("page:stale-vm"));
    assert!(provider
        .page_control_sessions
        .contains_key("page:capacity-reaped"));
}

#[test]
fn status_reaps_isolated_vm_session_when_control_socket_is_gone() {
    let mut provider = BrowserEngineAdapter::new();
    provider.page_control_sessions.insert(
        "page:dead-vm".to_string(),
        PageControlSession {
            generation: "sha256:test-generation".to_string(),
            stream_id: "stream:proof:test".to_string(),
            socket_path: format!(
                "/tmp/elastos-browser-vm-dead-{}-{}.sock",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ),
            shutdown_socket_path: None,
            adapter_id: "mac-vm-product".to_string(),
            principal_id: Some("person:local:test".to_string()),
            engine: AdapterKind::ChromiumMicrovm,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::MechanismMicrovm,
            isolated_session: true,
            isolation_session_dir: Some("/tmp/elastos-browser-vm-sessions/dead-vm".to_string()),
            isolation_kind: Some("per_launch_vm_target".to_string()),
            process: None,
        },
    );

    let status = serde_json::to_value(provider.status(None)).unwrap();

    assert_eq!(status["status"], "ok");
    assert_eq!(status["data"]["stale_sessions_reaped"], 1);
    assert_eq!(status["data"]["active_sessions"], 0);
    assert!(provider.page_control_sessions.is_empty());
}

#[test]
fn launch_reconciliation_recovers_exact_control_service_effect_after_provider_restart() {
    let generation = "sha256:provider-restart-reconciliation";
    let stream_id = "stream:provider-restart-reconciliation";
    let control_socket = spawn_status_socket(json!({
        "schema": "elastos.browser.vm-control-service.launch-reconciliation/v1",
        "state": "effect_acquired",
        "launch": {
            "adapter": "browser-vm-product",
            "engine": "chromium_microvm",
            "lifecycle_generation": generation,
            "stream_id": stream_id,
            "principal_id": "person:local:test",
            "display_mode": "webrtc_remote_display",
            "guarantee_level": "mechanism_microvm"
        },
        "effects": {
            "page_acquired": true,
            "vm_acquired": true
        },
        "supervisor_result": {
            "schema": "elastos.browser.engine.supervisor-result/v1",
            "page_id": "page:provider-restart-reconciliation",
            "adapter": "browser-vm-product",
            "engine": "chromium_microvm",
            "stream_id": stream_id,
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "wallet_injection": false,
            "control_socket_path": "/tmp/elastos-browser-provider-restart-guest.sock",
            "isolated_session": true,
            "isolation": {
                "schema": "elastos.browser.engine.isolation/v1",
                "kind": "per_launch_vm_target",
                "session_dir": "/tmp/elastos-browser-provider-restart-session"
            },
            "process": {
                "pid": 42,
                "stream_bridge_pid": null
            },
            "display_session": {
                "schema": "elastos.browser.display-session/v1",
                "session_id": "display:provider-restart-reconciliation",
                "mode": "webrtc_remote_display",
                "network_mode": "runtime_net_only",
                "direct_network": false,
                "input": "datachannel",
                "width": 1280,
                "height": 720,
                "display_backend": "vm_selkies_gstreamer_webrtc",
                "backend_class": "product_compositor",
                "media_transport": "runtime_relay",
                "audio": true,
                "video": true,
                "signaling_url": "/api/apps/browser/pages/page%3Aprovider-restart-reconciliation/webrtc"
            }
        }
    }));
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(json!({
        "adapters": [{
            "id": "browser-vm-product",
            "kind": "chromium_microvm",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/false",
                "timeout_ms": 2000,
                "control_socket_path": control_socket
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");
    assert!(provider.page_control_sessions.is_empty());

    let response = serde_json::to_value(provider.reconcile_launch(
        Some("person:local:test".to_string()),
        generation,
        stream_id,
    ))
    .unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["state"], "effect_acquired");
    assert_eq!(
        response["data"]["effect"]["page_id"],
        "page:provider-restart-reconciliation"
    );
    assert_eq!(
        response["data"]["effect"]["runtime_cleanup"]["generation"],
        generation
    );
    assert_eq!(
        response["data"]["effect"]["runtime_cleanup"]["stream_id"],
        stream_id
    );
    assert!(provider
        .page_control_sessions
        .contains_key("page:provider-restart-reconciliation"));
}

#[test]
fn launch_reconciliation_did_not_act_requires_every_control_service() {
    let generation = "sha256:all-services-proof";
    let stream_id = "stream:all-services-proof";
    let did_not_act = spawn_status_socket(json!({
        "schema": "elastos.browser.vm-control-service.launch-reconciliation/v1",
        "state": "did_not_act",
        "launch": {
            "lifecycle_generation": generation,
            "stream_id": stream_id
        },
        "effects": {
            "page_acquired": false,
            "vm_acquired": false
        }
    }));
    let indeterminate = spawn_status_socket(json!({
        "schema": "elastos.browser.vm-control-service.launch-reconciliation/v1",
        "state": "indeterminate",
        "launch": {
            "lifecycle_generation": generation,
            "stream_id": stream_id
        }
    }));
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(json!({
        "adapters": [
            {
                "id": "browser-vm-a",
                "kind": "chromium_microvm",
                "display_modes": ["webrtc_remote_display"],
                "supervisor": {
                    "program": "/bin/false",
                    "timeout_ms": 2000,
                    "control_socket_path": did_not_act
                }
            },
            {
                "id": "browser-vm-b",
                "kind": "chromium_microvm",
                "display_modes": ["webrtc_remote_display"],
                "supervisor": {
                    "program": "/bin/false",
                    "timeout_ms": 2000,
                    "control_socket_path": indeterminate
                }
            }
        ]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let response =
        serde_json::to_value(provider.reconcile_launch(None, generation, stream_id)).unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["state"], "cleanup_pending");
}

#[test]
fn launch_reconciliation_accepts_exact_unanimous_did_not_act_proof() {
    let generation = "sha256:did-not-act-proof";
    let stream_id = "stream:did-not-act-proof";
    let control_socket = spawn_status_socket(json!({
        "schema": "elastos.browser.vm-control-service.launch-reconciliation/v1",
        "state": "did_not_act",
        "launch": {
            "lifecycle_generation": generation,
            "stream_id": stream_id
        },
        "effects": {
            "page_acquired": false,
            "vm_acquired": false
        }
    }));
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(json!({
        "adapters": [{
            "id": "browser-vm-product",
            "kind": "chromium_microvm",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/false",
                "timeout_ms": 2000,
                "control_socket_path": control_socket
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let response =
        serde_json::to_value(provider.reconcile_launch(None, generation, stream_id)).unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["state"], "did_not_act");
    assert_eq!(response["data"]["effects"]["page_acquired"], false);
    assert_eq!(response["data"]["effects"]["vm_acquired"], false);
}

#[test]
fn launch_reconciliation_accepts_restart_persistent_terminal_cleanup_proof() {
    let generation = "sha256:restart-terminal-proof";
    let stream_id = "stream:restart-terminal-proof";
    let control_socket = spawn_status_socket(json!({
        "schema": "elastos.browser.vm-control-service.launch-reconciliation/v1",
        "state": "terminal_post_effect_cleanup",
        "launch": {
            "lifecycle_generation": generation,
            "stream_id": stream_id
        },
        "effects": {
            "page_acquired": true,
            "vm_acquired": true
        }
    }));
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(json!({
        "adapters": [{
            "id": "browser-vm-product",
            "kind": "chromium_microvm",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/false",
                "timeout_ms": 2000,
                "control_socket_path": control_socket
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let response =
        serde_json::to_value(provider.reconcile_launch(None, generation, stream_id)).unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["state"], "terminal_post_effect_cleanup");
    assert_eq!(response["data"]["effects"]["page_acquired"], true);
    assert_eq!(response["data"]["effects"]["vm_acquired"], true);
}

#[test]
fn launch_reconciliation_is_bounded_when_control_service_is_unresponsive() {
    let control_socket = spawn_unresponsive_status_socket();
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(json!({
        "adapters": [{
            "id": "browser-vm-product",
            "kind": "chromium_microvm",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/false",
                "timeout_ms": 2000,
                "control_socket_path": control_socket
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let started = Instant::now();
    let response = serde_json::to_value(provider.reconcile_launch(
        None,
        "sha256:unresponsive-reconciliation",
        "stream:unresponsive-reconciliation",
    ))
    .unwrap();

    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["state"], "cleanup_pending");
}

#[test]
fn init_rejects_invalid_session_capacity() {
    let mut provider = BrowserEngineAdapter::new();
    assert_eq!(
        error_code(provider.init(json!({
            "max_active_sessions": 0,
            "adapters": [{
                "id": "hosted-product",
                "kind": "selkies_gstreamer",
                "display_modes": ["webrtc_remote_display"],
                "supervisor": {
                    "program": "/bin/true",
                    "timeout_ms": 2000
                }
            }]
        }))),
        "invalid_config"
    );
}

#[test]
fn page_operations_fail_without_page_scoped_control_session() {
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(json!({
        "adapters": [{
            "id": "hosted-product",
            "kind": "selkies_gstreamer",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/true",
                "timeout_ms": 2000,
                "control_socket_path": "/tmp/elastos-global-control-socket.sock"
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    assert_eq!(
        error_code(provider.page_status("page:not-launched", None)),
        "engine_process_unavailable"
    );
    assert_eq!(
        error_code(provider.diagnostics("page:not-launched", None)),
        "engine_process_unavailable"
    );
    assert_eq!(
        error_code(provider.input("page:not-launched", json!({"type": "click"}), None)),
        "engine_process_unavailable"
    );
}

#[test]
fn close_page_missing_adapter_map_with_live_child_remains_indeterminate() {
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("5")
        .spawn()
        .expect("test child");
    let socket_path = format!(
        "/tmp/elastos-browser-missing-map-{}-{}.sock",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let binding = EngineCleanupBinding {
        schema: BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA.to_string(),
        page_id: "page:missing-map-live-child".to_string(),
        generation: "sha256:missing-map-live-child".to_string(),
        stream_id: "stream:missing-map-live-child".to_string(),
        adapter: "hosted-product".to_string(),
        engine: AdapterKind::SelkiesGstreamer,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
        guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
        principal_id: Some("person:local:test".to_string()),
        control_socket_path: socket_path,
        shutdown_socket_path: None,
        isolated_session: false,
        isolation: None,
        process: Some(json!({"pid": child.id(), "stream_bridge_pid": null})),
    };
    let mut provider = BrowserEngineAdapter::new();

    assert_eq!(
        error_code(provider.close_page(
            "page:missing-map-live-child",
            Some("person:local:test".to_string()),
            binding,
        )),
        "engine_process_unavailable"
    );
    assert!(
        child.try_wait().unwrap().is_none(),
        "missing adapter state must not synthesize terminal child cleanup"
    );
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn exact_typed_already_absent_supervisor_proof_is_terminal() {
    let session = PageControlSession {
        generation: "sha256:typed-absent".to_string(),
        stream_id: "stream:typed-absent".to_string(),
        socket_path: "/tmp/elastos-browser-typed-absent.sock".to_string(),
        shutdown_socket_path: None,
        adapter_id: "hosted-product".to_string(),
        principal_id: Some("person:local:test".to_string()),
        engine: AdapterKind::SelkiesGstreamer,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
        guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
        isolated_session: false,
        isolation_session_dir: None,
        isolation_kind: None,
        process: None,
    };
    let binding = engine_cleanup_binding("page:typed-absent", &session);
    let receipt = json!({
        "schema": BROWSER_SUPERVISOR_CLEANUP_RESULT_SCHEMA,
        "page_id": binding.page_id,
        "generation": binding.generation,
        "binding": binding,
        "terminal": true,
        "effects": {
            "page_absent": true,
            "child_absent": true,
            "vm_absent": true,
            "route_absent": true,
            "socket_absent": true
        }
    });

    let terminal = engine_terminal_cleanup_result(&binding, receipt).unwrap();
    assert_eq!(terminal["schema"], BROWSER_ENGINE_CLEANUP_RESULT_SCHEMA);
    assert_eq!(terminal["terminal"], true);
}

#[test]
fn page_operations_reject_mismatched_principal() {
    let mut provider = BrowserEngineAdapter::new();
    provider.page_control_sessions.insert(
        "page:owned".to_string(),
        PageControlSession {
            generation: "sha256:test-generation".to_string(),
            stream_id: "stream:proof:test".to_string(),
            socket_path: "/tmp/elastos-browser-owned-unused.sock".to_string(),
            shutdown_socket_path: None,
            adapter_id: "hosted-product".to_string(),
            principal_id: Some("person:local:owner".to_string()),
            engine: AdapterKind::SelkiesGstreamer,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
            isolated_session: false,
            isolation_session_dir: None,
            isolation_kind: None,
            process: None,
        },
    );

    let other = Some("person:local:other".to_string());
    assert_eq!(
        error_code(provider.page_status("page:owned", other.clone())),
        "page_not_found"
    );
    assert_eq!(
        error_code(provider.diagnostics("page:owned", other.clone())),
        "page_not_found"
    );
    assert_eq!(
        error_code(provider.input("page:owned", json!({"type": "click"}), other.clone())),
        "page_not_found"
    );
    assert_eq!(
        error_code(provider.webrtc_signal(
            "page:owned",
            json!({
                "schema": "elastos.browser.webrtc-offer/v1",
                "type": "offer",
                "sdp": "v=0\r\ns=ElastOS Browser Test\r\n"
            }),
            Some("video".to_string()),
            other.clone(),
        )),
        "page_not_found"
    );
    let cleanup = cleanup_binding_for(&provider, "page:owned");
    assert_eq!(
        error_code(provider.close_page("page:owned", other, cleanup)),
        "page_not_found"
    );
    assert!(provider.page_control_sessions.contains_key("page:owned"));
}

#[test]
fn close_page_retains_non_isolated_session_when_close_fails() {
    let mut provider = BrowserEngineAdapter::new();
    let socket_path = "/tmp/elastos-browser-close-missing.sock";
    let _ = std::fs::remove_file(socket_path);
    provider.page_control_sessions.insert(
        "page:retry-close".to_string(),
        PageControlSession {
            generation: "sha256:test-generation".to_string(),
            stream_id: "stream:proof:test".to_string(),
            socket_path: socket_path.to_string(),
            shutdown_socket_path: None,
            adapter_id: "hosted-product".to_string(),
            principal_id: Some("person:local:test".to_string()),
            engine: AdapterKind::SelkiesGstreamer,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
            isolated_session: false,
            isolation_session_dir: None,
            isolation_kind: None,
            process: None,
        },
    );

    let cleanup = cleanup_binding_for(&provider, "page:retry-close");
    assert_eq!(
        error_code(provider.close_page(
            "page:retry-close",
            Some("person:local:test".to_string()),
            cleanup,
        )),
        "engine_process_unavailable"
    );
    assert!(provider
        .page_control_sessions
        .contains_key("page:retry-close"));
}

#[test]
fn close_page_retains_isolated_session_when_shutdown_and_cleanup_fail() {
    let mut provider = BrowserEngineAdapter::new();
    let socket_path = "/tmp/elastos-browser-isolated-close-missing.sock";
    let _ = std::fs::remove_file(socket_path);
    provider.page_control_sessions.insert(
        "page:retry-isolated-close".to_string(),
        PageControlSession {
            generation: "sha256:test-generation".to_string(),
            stream_id: "stream:proof:test".to_string(),
            socket_path: socket_path.to_string(),
            shutdown_socket_path: Some(socket_path.to_string()),
            adapter_id: "hosted-product".to_string(),
            principal_id: Some("person:local:test".to_string()),
            engine: AdapterKind::SelkiesGstreamer,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
            isolated_session: true,
            isolation_session_dir: Some("/tmp/not-an-elastos-browser-session".to_string()),
            isolation_kind: Some("per_launch_selkies_target".to_string()),
            process: None,
        },
    );

    let cleanup = cleanup_binding_for(&provider, "page:retry-isolated-close");
    assert_eq!(
        error_code(provider.close_page(
            "page:retry-isolated-close",
            Some("person:local:test".to_string()),
            cleanup,
        )),
        "engine_process_unavailable"
    );
    assert!(provider
        .page_control_sessions
        .contains_key("page:retry-isolated-close"));
}

#[test]
fn page_status_includes_redacted_engine_identity() {
    let status_socket = spawn_status_socket(json!({
        "schema": "elastos.browser.page-status/v1",
        "actual_url": "https://ela.city/channels",
        "title": "ela.city",
        "direct_network": false,
        "display_session": {
            "schema": "elastos.browser.display-session/v1",
            "mode": "webrtc_remote_display",
            "media_transport": "runtime_relay",
            "display_backend": "selkies_gstreamer_webrtc",
            "backend_class": "product_compositor",
            "network_mode": "runtime_net_only",
            "direct_network": false
        }
    }));
    let mut provider = BrowserEngineAdapter::new();
    provider.page_control_sessions.insert(
        "page:browser-vm-status".to_string(),
        PageControlSession {
            generation: "sha256:test-generation".to_string(),
            stream_id: "stream:proof:test".to_string(),
            socket_path: status_socket,
            shutdown_socket_path: None,
            adapter_id: "browser-vm-product".to_string(),
            principal_id: Some("person:local:test".to_string()),
            engine: AdapterKind::ChromiumMicrovm,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::MechanismMicrovm,
            isolated_session: true,
            isolation_session_dir: Some("/tmp/elastos-browser-vm-sessions/status".to_string()),
            isolation_kind: Some("per_launch_vm_target".to_string()),
            process: None,
        },
    );

    let response = serde_json::to_value(provider.page_status(
        "page:browser-vm-status",
        Some("person:local:test".to_string()),
    ))
    .unwrap();
    assert_eq!(response["status"], "ok");
    let identity = &response["data"]["engine_identity"];
    assert_eq!(identity["schema"], "elastos.browser.engine.identity/v1");
    assert_eq!(identity["adapter"], "browser-vm-product");
    assert_eq!(identity["engine"], "chromium_microvm");
    assert_eq!(identity["display_mode"], "webrtc_remote_display");
    assert_eq!(identity["guarantee_level"], "mechanism_microvm");
    assert_eq!(identity["engine_control"], "page_scoped");
    assert_eq!(identity["isolated_engine_session"], true);
    assert_eq!(identity["isolation_kind"], "per_launch_vm_target");
    assert!(identity.get("socket_path").is_none());
    assert!(identity.get("shutdown_socket_path").is_none());
    assert!(identity.get("session_dir").is_none());
}

#[test]
fn isolated_close_uses_target_shutdown_contract() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::thread;

    let socket_path = format!(
        "/tmp/elastos-browser-isolated-close-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path).unwrap();
    let handle = thread::spawn({
        let socket_path = socket_path.clone();
        move || {
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 256];
                loop {
                    let size = stream.read(&mut buffer).unwrap_or(0);
                    if size == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..size]);
                    if http_request_is_complete(&request) {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                if !request.starts_with("POST /shutdown HTTP/1.1") {
                    let body = r#"{"schema":"elastos.browser.test-probe/v1","ok":true}"#;
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .unwrap();
                    continue;
                }
                let body = typed_supervisor_cleanup_receipt(&request);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
                let _ = std::fs::remove_file(socket_path);
                return;
            }
            panic!("test listener did not receive POST /shutdown");
        }
    });

    let mut provider = BrowserEngineAdapter::new();
    provider.page_control_sessions.insert(
        "page:isolated-close".to_string(),
        PageControlSession {
            generation: "sha256:test-generation".to_string(),
            stream_id: "stream:proof:test".to_string(),
            socket_path: socket_path.clone(),
            shutdown_socket_path: None,
            adapter_id: "hosted-product".to_string(),
            principal_id: Some("person:local:test".to_string()),
            engine: AdapterKind::SelkiesGstreamer,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
            isolated_session: true,
            isolation_session_dir: Some(
                "/tmp/elastos-browser-sessions/stream_isolated-close-test".to_string(),
            ),
            isolation_kind: Some("per_launch_selkies_target".to_string()),
            process: None,
        },
    );

    let cleanup = cleanup_binding_for(&provider, "page:isolated-close");
    let response = serde_json::to_value(provider.close_page(
        "page:isolated-close",
        Some("person:local:test".to_string()),
        cleanup,
    ))
    .unwrap();
    assert_eq!(response["status"], "ok");
    assert_eq!(
        response["data"]["schema"],
        BROWSER_ENGINE_CLEANUP_RESULT_SCHEMA
    );
    assert_eq!(response["data"]["terminal"], true);
    assert_eq!(response["data"]["effects"]["child_absent"], true);
    handle.join().unwrap();
}

#[test]
fn vm_isolated_close_uses_global_shutdown_socket() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::thread;
    use std::time::Duration;

    let socket_path = format!(
        "/tmp/elastos-browser-vm-close-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path).unwrap();
    let handle = thread::spawn({
        let socket_path = socket_path.clone();
        move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 256];
            loop {
                let size = stream.read(&mut buffer).unwrap_or(0);
                if size == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..size]);
                if http_request_is_complete(&request) {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request);
            assert!(request.starts_with("POST /shutdown HTTP/1.1"));
            assert!(request.contains("\"page_id\":\"page:browser-vm-close\""));
            let body = typed_supervisor_cleanup_receipt(&request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            let _ = std::fs::remove_file(socket_path);
        }
    });

    let mut provider = BrowserEngineAdapter::new();
    provider.page_control_sessions.insert(
        "page:browser-vm-close".to_string(),
        PageControlSession {
            generation: "sha256:test-generation".to_string(),
            stream_id: "stream:proof:test".to_string(),
            socket_path: "/tmp/elastos-browser-vm-page-control-not-used.sock".to_string(),
            shutdown_socket_path: Some(socket_path),
            adapter_id: "browser-vm-product".to_string(),
            principal_id: Some("person:local:test".to_string()),
            engine: AdapterKind::ChromiumMicrovm,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
            guarantee_level: BrowserGuaranteeLevel::MechanismMicrovm,
            isolated_session: true,
            isolation_session_dir: Some("/tmp/evzl/bvm-vm-close-test".to_string()),
            isolation_kind: Some("per_launch_vm_target".to_string()),
            process: None,
        },
    );

    let cleanup = cleanup_binding_for(&provider, "page:browser-vm-close");
    let response = serde_json::to_value(provider.close_page(
        "page:browser-vm-close",
        Some("person:local:test".to_string()),
        cleanup,
    ))
    .unwrap();
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["terminal"], true);
    assert_eq!(response["data"]["effects"]["vm_absent"], true);
    handle.join().unwrap();
}

#[test]
fn supervisor_accepts_vm_isolation_kind() {
    let result = SupervisorLaunchResult {
        schema: "elastos.browser.engine.supervisor-result/v1".to_string(),
        page_id: "page:browser-vm".to_string(),
        adapter: "browser-vm-product".to_string(),
        engine: AdapterKind::ChromiumMicrovm,
        stream_id: "stream:proof:test".to_string(),
        actual_url: None,
        title: None,
        network_mode: AdapterNetworkMode::RuntimeNetOnly,
        direct_network: false,
        wallet_injection: false,
        display_session: json!({
            "schema": "elastos.browser.display-session/v1",
            "session_id": "display:stream:proof:test",
            "mode": "webrtc_remote_display",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "input": "datachannel",
            "width": 1280,
            "height": 720,
            "display_backend": "vm_selkies_gstreamer_webrtc",
            "backend_class": "product_compositor",
            "media_transport": "runtime_relay",
            "audio": true,
            "video": true,
            "signaling_url": "/api/apps/browser/pages/page%3Abrowser-vm/webrtc"
        }),
        view: None,
        wallet_bridge: None,
        control_socket_path: Some("/tmp/elastos-browser-vm.sock".to_string()),
        isolated_session: true,
        isolation: Some(SupervisorIsolation {
            schema: "elastos.browser.engine.isolation/v1".to_string(),
            kind: "per_launch_vm_target".to_string(),
            session_dir: "/tmp/elastos-browser-vm-sessions/test".to_string(),
        }),
        process: Some(json!({"pid": 42, "stream_bridge_pid": null})),
    };
    let adapter = AdapterConfig {
        id: "browser-vm-product".to_string(),
        kind: AdapterKind::ChromiumMicrovm,
        network_mode: AdapterNetworkMode::RuntimeNetOnly,
        display_modes: vec![BrowserDisplayMode::WebrtcRemoteDisplay],
        supervisor: None,
    };

    validate_supervisor_result(
        &result,
        &adapter,
        "stream:proof:test",
        BrowserDisplayMode::WebrtcRemoteDisplay,
    )
    .unwrap();
}

#[test]
fn supervisor_rejects_video_only_vm_product_display() {
    let result = SupervisorLaunchResult {
        schema: "elastos.browser.engine.supervisor-result/v1".to_string(),
        page_id: "page:browser-vm".to_string(),
        adapter: "browser-vm-product".to_string(),
        engine: AdapterKind::ChromiumMicrovm,
        stream_id: "stream:proof:test".to_string(),
        actual_url: None,
        title: None,
        network_mode: AdapterNetworkMode::RuntimeNetOnly,
        direct_network: false,
        wallet_injection: false,
        display_session: json!({
            "schema": "elastos.browser.display-session/v1",
            "session_id": "display:stream:proof:test",
            "mode": "webrtc_remote_display",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "input": "datachannel",
            "width": 1280,
            "height": 720,
            "display_backend": "vm_selkies_gstreamer_webrtc",
            "backend_class": "product_compositor",
            "media_transport": "runtime_relay",
            "audio": false,
            "video": true,
            "signaling_url": "/api/apps/browser/pages/page%3Abrowser-vm/webrtc"
        }),
        view: None,
        wallet_bridge: None,
        control_socket_path: Some("/tmp/elastos-browser-vm.sock".to_string()),
        isolated_session: true,
        isolation: Some(SupervisorIsolation {
            schema: "elastos.browser.engine.isolation/v1".to_string(),
            kind: "per_launch_vm_target".to_string(),
            session_dir: "/tmp/elastos-browser-vm-sessions/test".to_string(),
        }),
        process: Some(json!({"pid": 42, "stream_bridge_pid": null})),
    };
    let adapter = AdapterConfig {
        id: "browser-vm-product".to_string(),
        kind: AdapterKind::ChromiumMicrovm,
        network_mode: AdapterNetworkMode::RuntimeNetOnly,
        display_modes: vec![BrowserDisplayMode::WebrtcRemoteDisplay],
        supervisor: None,
    };

    let err = validate_supervisor_result(
        &result,
        &adapter,
        "stream:proof:test",
        BrowserDisplayMode::WebrtcRemoteDisplay,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "Browser VM product display sessions must advertise audio=true and video=true"
    );
}

#[test]
fn supervisor_rejects_vm_product_display_without_video() {
    let result = SupervisorLaunchResult {
        schema: "elastos.browser.engine.supervisor-result/v1".to_string(),
        page_id: "page:browser-vm".to_string(),
        adapter: "browser-vm-product".to_string(),
        engine: AdapterKind::ChromiumMicrovm,
        stream_id: "stream:proof:test".to_string(),
        actual_url: None,
        title: None,
        network_mode: AdapterNetworkMode::RuntimeNetOnly,
        direct_network: false,
        wallet_injection: false,
        display_session: json!({
            "schema": "elastos.browser.display-session/v1",
            "session_id": "display:stream:proof:test",
            "mode": "webrtc_remote_display",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "input": "datachannel",
            "width": 1280,
            "height": 720,
            "display_backend": "vm_selkies_gstreamer_webrtc",
            "backend_class": "product_compositor",
            "media_transport": "runtime_relay",
            "audio": false,
            "video": false,
            "signaling_url": "/api/apps/browser/pages/page%3Abrowser-vm/webrtc"
        }),
        view: None,
        wallet_bridge: None,
        control_socket_path: Some("/tmp/elastos-browser-vm.sock".to_string()),
        isolated_session: true,
        isolation: Some(SupervisorIsolation {
            schema: "elastos.browser.engine.isolation/v1".to_string(),
            kind: "per_launch_vm_target".to_string(),
            session_dir: "/tmp/elastos-browser-vm-sessions/test".to_string(),
        }),
        process: Some(json!({"pid": 42, "stream_bridge_pid": null})),
    };
    let adapter = AdapterConfig {
        id: "browser-vm-product".to_string(),
        kind: AdapterKind::ChromiumMicrovm,
        network_mode: AdapterNetworkMode::RuntimeNetOnly,
        display_modes: vec![BrowserDisplayMode::WebrtcRemoteDisplay],
        supervisor: None,
    };

    let err = validate_supervisor_result(
        &result,
        &adapter,
        "stream:proof:test",
        BrowserDisplayMode::WebrtcRemoteDisplay,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "Browser VM product display sessions must advertise audio=true and video=true"
    );
}

#[test]
fn supervisor_rejects_non_relay_vm_media_transport() {
    let result = SupervisorLaunchResult {
        schema: "elastos.browser.engine.supervisor-result/v1".to_string(),
        page_id: "page:browser-vm".to_string(),
        adapter: "browser-vm-product".to_string(),
        engine: AdapterKind::ChromiumMicrovm,
        stream_id: "stream:proof:test".to_string(),
        actual_url: None,
        title: None,
        network_mode: AdapterNetworkMode::RuntimeNetOnly,
        direct_network: false,
        wallet_injection: false,
        display_session: json!({
            "schema": "elastos.browser.display-session/v1",
            "session_id": "display:stream:proof:test",
            "mode": "webrtc_remote_display",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "input": "datachannel",
            "input_protocol": "selkies_v1",
            "width": 1280,
            "height": 720,
            "display_backend": "vm_selkies_gstreamer_webrtc",
            "backend_class": "product_compositor",
            "media_transport": "host_media",
            "audio": true,
            "video": true,
            "signaling_url": "/api/apps/browser/pages/page%3Abrowser-vm/webrtc"
        }),
        view: None,
        wallet_bridge: None,
        control_socket_path: Some("/tmp/elastos-browser-vm.sock".to_string()),
        isolated_session: true,
        isolation: Some(SupervisorIsolation {
            schema: "elastos.browser.engine.isolation/v1".to_string(),
            kind: "per_launch_vm_target".to_string(),
            session_dir: "/tmp/elastos-browser-vm-sessions/test".to_string(),
        }),
        process: Some(json!({"pid": 42, "stream_bridge_pid": null})),
    };
    let adapter = AdapterConfig {
        id: "browser-vm-product".to_string(),
        kind: AdapterKind::ChromiumMicrovm,
        network_mode: AdapterNetworkMode::RuntimeNetOnly,
        display_modes: vec![BrowserDisplayMode::WebrtcRemoteDisplay],
        supervisor: None,
    };

    let err = validate_supervisor_result(
        &result,
        &adapter,
        "stream:proof:test",
        BrowserDisplayMode::WebrtcRemoteDisplay,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "Browser VM display sessions must report media_transport=runtime_relay"
    );
}

#[test]
fn vm_isolation_requires_runtime_relay_media_transport() {
    let result = SupervisorLaunchResult {
        schema: "elastos.browser.engine.supervisor-result/v1".to_string(),
        page_id: "page:browser-vm".to_string(),
        adapter: "browser-vm-product".to_string(),
        engine: AdapterKind::ChromiumMicrovm,
        stream_id: "stream:proof:test".to_string(),
        actual_url: None,
        title: None,
        network_mode: AdapterNetworkMode::RuntimeNetOnly,
        direct_network: false,
        wallet_injection: false,
        display_session: json!({
            "schema": "elastos.browser.display-session/v1",
            "session_id": "display:stream:proof:test",
            "mode": "webrtc_remote_display",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "input": "datachannel",
            "width": 1280,
            "height": 720,
            "display_backend": "vm_selkies_gstreamer_webrtc",
            "backend_class": "product_compositor",
            "audio": true,
            "video": true,
            "signaling_url": "/api/apps/browser/pages/page%3Abrowser-vm/webrtc"
        }),
        view: None,
        wallet_bridge: None,
        control_socket_path: Some("/tmp/elastos-browser-vm.sock".to_string()),
        isolated_session: true,
        isolation: Some(SupervisorIsolation {
            schema: "elastos.browser.engine.isolation/v1".to_string(),
            kind: "per_launch_vm_target".to_string(),
            session_dir: "/tmp/elastos-browser-vm-sessions/test".to_string(),
        }),
        process: Some(json!({"pid": 42, "stream_bridge_pid": null})),
    };
    let adapter = AdapterConfig {
        id: "browser-vm-product".to_string(),
        kind: AdapterKind::ChromiumMicrovm,
        network_mode: AdapterNetworkMode::RuntimeNetOnly,
        display_modes: vec![BrowserDisplayMode::WebrtcRemoteDisplay],
        supervisor: None,
    };

    let err = validate_supervisor_result(
        &result,
        &adapter,
        "stream:proof:test",
        BrowserDisplayMode::WebrtcRemoteDisplay,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "Browser VM display sessions must report media_transport=runtime_relay"
    );
}

#[test]
fn webrtc_datachannel_display_requires_coordinate_size() {
    let err = validate_display_session(
        &json!({
            "schema": "elastos.browser.display-session/v1",
            "session_id": "display:stream:proof:test",
            "mode": "webrtc_remote_display",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "input": "datachannel",
            "display_backend": "selkies_gstreamer_webrtc",
            "backend_class": "product_compositor",
            "audio": true,
            "video": true,
            "signaling_url": "/api/apps/browser/pages/page%3Awebrtc-product-audio/webrtc"
        }),
        BrowserDisplayMode::WebrtcRemoteDisplay,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "datachannel WebRTC display sessions must report display width"
    );
}

#[test]
fn hosted_remote_browser_can_declare_product_compositor() {
    let mut provider = BrowserEngineAdapter::new();
    let script = r#"printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:hosted-remote-product","adapter":"hosted-product","engine":"hosted_remote_browser","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"webrtc_remote_display","network_mode":"runtime_net_only","direct_network":false,"input":"datachannel","width":1280,"height":720,"display_backend":"kasmvnc_webrtc","backend_class":"product_compositor","audio":true,"video":true,"signaling_url":"/api/apps/browser/pages/page%3Ahosted-remote-product/webrtc"}}'"#;
    let init = provider.init(json!({
        "adapters": [{
            "id": "hosted-product",
            "kind": "hosted_remote_browser",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/sh",
                "args": ["-c", script],
                "timeout_ms": 2000
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let response = serde_json::to_value(provider.launch_with_viewport(LaunchContext {
        url: "https://example.com/",
        stream_session: &stream_receipt("adapter_ipc"),
        profile: test_browser_profile(),
        adapter_id: None,
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
        guarantee_level: BrowserGuaranteeLevel::OperatorRbi,
    }))
    .unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["engine"], "hosted_remote_browser");
    assert_eq!(
        response["data"]["display_session"]["display_backend"],
        "kasmvnc_webrtc"
    );
    assert_eq!(response["data"]["display_session"]["audio"], true);
    assert_eq!(response["data"]["display_session"]["direct_network"], false);
}

#[test]
fn native_adapter_can_declare_native_surface_display_mode() {
    let mut provider = BrowserEngineAdapter::new();
    let script = r#"printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:native-surface-proof","adapter":"linux-cef","engine":"cef","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"native_surface","surface_id":"surface:native-proof","network_mode":"runtime_net_only","direct_network":false,"input":"native_ipc","width":1280,"height":720,"audio":true,"video":true},"view":{"schema":"elastos.browser.view/v1","mode":"native_surface","width":1280,"height":720}}'"#;
    let init = provider.init(json!({
        "adapters": [{
            "id": "linux-cef",
            "kind": "cef",
            "display_modes": ["native_surface"],
            "supervisor": {
                "program": "/bin/sh",
                "args": ["-c", script],
                "timeout_ms": 2000
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let response = serde_json::to_value(provider.launch_with_viewport(LaunchContext {
        url: "https://glidefinance.io/",
        stream_session: &stream_receipt("adapter_ipc"),
        profile: test_browser_profile(),
        adapter_id: None,
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::NativeSurface,
        guarantee_level: BrowserGuaranteeLevel::PolicyWebview,
    }))
    .unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["page_id"], "page:native-surface-proof");
    assert_eq!(
        response["data"]["display_session"]["mode"],
        "native_surface"
    );
    assert_eq!(
        response["data"]["display_session"]["surface_id"],
        "surface:native-proof"
    );
    assert_eq!(response["data"]["display_session"]["input"], "native_ipc");
    assert_eq!(response["data"]["display_session"]["width"], 1280);
    assert_eq!(response["data"]["display_session"]["height"], 720);
    assert_eq!(
        response["data"]["view"]["schema"],
        "elastos.browser.view/v1"
    );
    assert_eq!(response["data"]["view"]["mode"], "native_surface");
    assert_eq!(response["data"]["view"]["width"], 1280);
    assert_eq!(response["data"]["view"]["height"], 720);
}

#[test]
fn native_surface_supervisor_result_requires_view_geometry() {
    let result = SupervisorLaunchResult {
        schema: "elastos.browser.engine.supervisor-result/v1".to_string(),
        page_id: "page:native-surface-proof".to_string(),
        adapter: "linux-cef".to_string(),
        engine: AdapterKind::Cef,
        stream_id: "stream:proof:test".to_string(),
        actual_url: None,
        title: None,
        network_mode: AdapterNetworkMode::RuntimeNetOnly,
        direct_network: false,
        wallet_injection: false,
        display_session: json!({
            "schema": "elastos.browser.display-session/v1",
            "session_id": "display:stream:proof:test",
            "mode": "native_surface",
            "surface_id": "surface:native-proof",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "input": "native_ipc",
            "width": 1280,
            "height": 720,
            "audio": true,
            "video": true
        }),
        view: None,
        wallet_bridge: None,
        control_socket_path: None,
        isolated_session: false,
        isolation: None,
        process: None,
    };
    let adapter = AdapterConfig {
        id: "linux-cef".to_string(),
        kind: AdapterKind::Cef,
        network_mode: AdapterNetworkMode::RuntimeNetOnly,
        display_modes: vec![BrowserDisplayMode::NativeSurface],
        supervisor: None,
    };

    let err = validate_supervisor_result(
        &result,
        &adapter,
        "stream:proof:test",
        BrowserDisplayMode::NativeSurface,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "native_surface supervisor result omitted view geometry"
    );
}

#[test]
fn native_adapter_passes_operator_supervisor_env() {
    let mut provider = BrowserEngineAdapter::new();
    let script = r#"test "$ELASTOS_BROWSER_ENGINE_TEST_ENV" = "ok" && printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:native-proof","adapter":"linux-chromium-headless","engine":"chromium_headless","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"webrtc_remote_display","network_mode":"runtime_net_only","direct_network":false,"input":"runtime_route","audio":false,"video":true,"signaling_url":"/api/apps/browser/pages/page%3Anative-proof/webrtc"}}'"#;
    let init = provider.init(json!({
        "adapters": [{
            "id": "linux-chromium-headless",
            "kind": "chromium_headless",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/sh",
                "args": ["-c", script],
                "env": {
                    "ELASTOS_BROWSER_ENGINE_TEST_ENV": "ok"
                },
                "timeout_ms": 2000
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let response = serde_json::to_value(provider.launch(
        "https://glidefinance.io/",
        &stream_receipt("adapter_ipc"),
        Some("person:local:test".to_string()),
        Some("open browser page".to_string()),
        json!({}),
    ))
    .unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["page_id"], "page:native-proof");
}

#[test]
fn native_adapter_rejects_supervisor_claiming_direct_network() {
    let mut provider = BrowserEngineAdapter::new();
    let script = r#"printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:native-proof","adapter":"linux-chromium-headless","engine":"chromium_headless","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":true,"wallet_injection":false,"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"webrtc_remote_display","network_mode":"runtime_net_only","direct_network":false,"input":"runtime_route","audio":false,"video":true,"signaling_url":"/api/apps/browser/pages/page%3Anative-proof/webrtc"}}'"#;
    let init = provider.init(json!({
        "adapters": [{
            "id": "linux-chromium-headless",
            "kind": "chromium_headless",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/sh",
                "args": ["-c", script],
                "timeout_ms": 2000
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    assert_eq!(
        error_code(provider.launch(
            "https://glidefinance.io/",
            &stream_receipt("adapter_ipc"),
            Some("person:local:test".to_string()),
            Some("open browser page".to_string()),
            json!({}),
        )),
        "invalid_supervisor_result"
    );
}

#[test]
fn request_decode_rejects_hidden_host_authority_fields() {
    let err = serde_json::from_value::<Request>(json!({
        "op": "launch",
        "url": "https://glidefinance.io/",
        "stream_session": {
            "schema": "elastos.exit.stream-session/v1",
            "stream_id": "stream:proof:test",
            "target": "tls://glidefinance.io:443",
            "byte_transport": "adapter_ipc"
        },
        "profile": test_browser_profile(),
        "raw_socket": true
    }))
    .unwrap_err()
    .to_string();

    assert!(err.contains("unknown field"));
}

#[test]
fn webrtc_signal_rejects_unsupported_payloads() {
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(json!({
        "adapters": [{
            "id": "linux-chromium-headless",
            "kind": "chromium_headless",
            "display_modes": ["webrtc_remote_display"],
            "supervisor": {
                "program": "/bin/false",
                "control_socket_path": "/tmp/elastos-browser-test.sock"
            }
        }]
    }));
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    let response = serde_json::to_value(provider.handle(Request::WebrtcSignal {
        page_id: "page:test".to_string(),
        channel: Some("video".to_string()),
        signal: json!({
            "schema": "elastos.browser.webrtc-unsupported/v1",
            "type": "unsupported"
        }),
        principal_id: Some("person:local:test".to_string()),
    }))
    .unwrap();

    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "invalid_request");
}

#[test]
fn webrtc_signal_validator_accepts_trickle_candidates() {
    let signal_type = validate_webrtc_signal(&json!({
        "schema": "elastos.browser.webrtc-candidate/v1",
        "type": "candidate",
        "candidate": {
            "candidate": "candidate:1 1 udp 2113937151 host.local 56929 typ host generation 0 network-cost 999",
            "sdpMid": "0",
            "sdpMLineIndex": 0
        }
    }))
    .unwrap();
    assert_eq!(signal_type, "candidate");

    let signal_type = validate_webrtc_signal(&json!({
        "schema": "elastos.browser.webrtc-end-of-candidates/v1",
        "type": "end_of_candidates"
    }))
    .unwrap();
    assert_eq!(signal_type, "end_of_candidates");
}

#[test]
fn webrtc_signal_validator_accepts_engine_offer_answer() {
    let signal_type = validate_webrtc_signal(&json!({
        "schema": "elastos.browser.webrtc-answer/v1",
        "type": "answer",
        "sdp": "v=0\r\ns=ElastOS Browser Test\r\n"
    }))
    .unwrap();
    assert_eq!(signal_type, "answer");
}

#[test]
fn webrtc_engine_offer_display_requires_initial_offer() {
    let err = validate_display_session(
        &json!({
            "schema": "elastos.browser.display-session/v1",
            "session_id": "display:stream:proof:test",
            "mode": "webrtc_remote_display",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "input": "datachannel",
            "width": 1280,
            "height": 720,
            "offerer": "engine",
            "display_backend": "selkies_gstreamer_webrtc",
            "backend_class": "product_compositor",
            "audio": true,
            "video": true,
            "signaling_url": "/api/apps/browser/pages/page%3Aengine-offer/webrtc"
        }),
        BrowserDisplayMode::WebrtcRemoteDisplay,
    )
    .unwrap_err();

    assert_eq!(
        err,
        "engine-offer WebRTC display sessions require initial_offer"
    );
}

#[test]
fn webrtc_signal_validator_rejects_candidates_inside_offer_sdp() {
    let err = validate_webrtc_signal(&json!({
        "schema": "elastos.browser.webrtc-offer/v1",
        "type": "offer",
        "sdp": "v=0\r\na=candidate:1 1 udp 2113937151 host.local 56929 typ host generation 0 network-cost 999\r\n"
    }))
    .unwrap_err();

    assert_eq!(
        err,
        "WebRTC offer must send ICE candidates through candidate messages"
    );
}

#[test]
fn webrtc_answer_validator_rejects_provider_errors() {
    let err = validate_webrtc_answer(&json!({
        "status": "error",
        "code": "engine_process_unavailable",
        "message": "browser page not found"
    }))
    .unwrap_err();

    assert_eq!(
        err,
        "WebRTC answer must use elastos.browser.webrtc-answer/v1"
    );
}
