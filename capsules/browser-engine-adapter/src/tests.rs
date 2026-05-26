use super::*;

fn error_code(response: Response) -> String {
    serde_json::to_value(response).unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string()
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

fn proof_adapter_config() -> Value {
    json!({
        "adapters": [{
            "id": "linux-proof",
            "kind": "contract_proof",
            "display_modes": ["diagnostic_frame"]
        }]
    })
}

#[test]
fn status_is_unavailable_without_configured_adapter() {
    let provider = BrowserEngineAdapter::new();
    let response =
        serde_json::to_value(provider.status(Some("person:local:test".to_string()))).unwrap();
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["status"], "unavailable");
    assert_eq!(response["data"]["direct_network"], false);
    assert_eq!(response["data"]["wallet_injection"], false);
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
        "diagnostic_frame"
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
fn launch_rejects_unimplemented_product_display_modes() {
    let mut provider = BrowserEngineAdapter::new();
    let init = provider.init(proof_adapter_config());
    assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

    assert_eq!(
        error_code(provider.launch_with_viewport(LaunchContext {
            url: "https://glidefinance.io/",
            stream_session: &stream_receipt("adapter_ipc"),
            principal_id: None,
            reason: None,
            wallet: json!({}),
            viewport: None,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
        })),
        "display_session_unavailable"
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
        "diagnostic_frame"
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
    let script = r#"printf '%s' "$ELASTOS_BROWSER_ENGINE_REQUEST" | grep -q '"principal_id":"person:local:test"' || exit 7; printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:native-proof","adapter":"linux-chromium-headless","engine":"chromium_headless","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"diagnostic_frame","network_mode":"runtime_net_only","direct_network":false,"input":"runtime_route","audio":false,"video":false}}'"#;
    let init = provider.init(json!({
        "adapters": [{
            "id": "linux-chromium-headless",
            "kind": "chromium_headless",
            "display_modes": ["diagnostic_frame"],
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
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
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
            principal_id: Some("person:local:test".to_string()),
            reason: Some("open browser page".to_string()),
            wallet: json!({}),
            viewport: None,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
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
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
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
    let script = r#"printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:isolated-product","adapter":"hosted-product","engine":"selkies_gstreamer","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"control_socket_path":"/tmp/elastos-browser-isolated-product.sock","isolated_session":true,"isolation":{"schema":"elastos.browser.engine.isolation/v1","kind":"per_launch_selkies_target","session_dir":"/tmp/elastos-browser-sessions/test"},"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"webrtc_remote_display","network_mode":"runtime_net_only","direct_network":false,"input":"datachannel","width":1280,"height":720,"display_backend":"selkies_gstreamer_webrtc","backend_class":"product_compositor","audio":true,"video":true,"signaling_url":"/api/apps/browser/pages/page%3Aisolated-product/webrtc"}}'"#;
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
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
    }))
    .unwrap();

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["engine_control"], "page_scoped");
    assert_eq!(response["data"]["isolated_engine_session"], true);
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
    let script = r#"printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:capacity-a","adapter":"hosted-product","engine":"selkies_gstreamer","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"control_socket_path":"/tmp/elastos-browser-capacity-a.sock","isolated_session":true,"isolation":{"schema":"elastos.browser.engine.isolation/v1","kind":"per_launch_selkies_target","session_dir":"/tmp/elastos-browser-sessions/capacity-a"},"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"webrtc_remote_display","network_mode":"runtime_net_only","direct_network":false,"input":"datachannel","width":1280,"height":720,"display_backend":"selkies_gstreamer_webrtc","backend_class":"product_compositor","audio":true,"video":true,"signaling_url":"/api/apps/browser/pages/page%3Acapacity-a/webrtc"}}'"#;
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
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
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
            principal_id: Some("person:local:test".to_string()),
            reason: Some("open second browser page".to_string()),
            wallet: json!({}),
            viewport: None,
            display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
        })),
        "browser_capacity_unavailable"
    );
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
        error_code(provider.input("page:not-launched", json!({"type": "click"}), None)),
        "engine_process_unavailable"
    );
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
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let size = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..size]);
            assert!(request.starts_with("POST /shutdown HTTP/1.1"));
            let body = r#"{"schema":"elastos.browser.selkies-control.shutdown/v1","ok":true}"#;
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
        "page:isolated-close".to_string(),
        PageControlSession {
            socket_path: socket_path.clone(),
            isolated_session: true,
            isolation_session_dir: Some(
                "/tmp/elastos-browser-sessions/stream_isolated-close-test".to_string(),
            ),
        },
    );

    let response = serde_json::to_value(
        provider.close_page("page:isolated-close", Some("person:local:test".to_string())),
    )
    .unwrap();
    assert_eq!(response["status"], "ok");
    assert_eq!(
        response["data"]["schema"],
        "elastos.browser.close-result/v1"
    );
    assert_eq!(response["data"]["closed"], true);
    assert_eq!(response["data"]["isolated_session"], true);
    assert_eq!(
        response["data"]["shutdown"]["schema"],
        "elastos.browser.selkies-control.shutdown/v1"
    );
    handle.join().unwrap();
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
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::WebrtcRemoteDisplay,
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
    let script = r#"printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:native-surface-proof","adapter":"linux-cef","engine":"cef","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"native_surface","surface_id":"surface:native-proof","network_mode":"runtime_net_only","direct_network":false,"input":"native_ipc","audio":true,"video":true}}'"#;
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
        principal_id: Some("person:local:test".to_string()),
        reason: Some("open browser page".to_string()),
        wallet: json!({}),
        viewport: None,
        display_mode: BrowserDisplayMode::NativeSurface,
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
}

#[test]
fn native_adapter_passes_operator_supervisor_env() {
    let mut provider = BrowserEngineAdapter::new();
    let script = r#"test "$ELASTOS_BROWSER_ENGINE_TEST_ENV" = "ok" && printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:native-proof","adapter":"linux-chromium-headless","engine":"chromium_headless","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":false,"wallet_injection":false,"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"diagnostic_frame","network_mode":"runtime_net_only","direct_network":false,"input":"runtime_route","audio":false,"video":false}}'"#;
    let init = provider.init(json!({
        "adapters": [{
            "id": "linux-chromium-headless",
            "kind": "chromium_headless",
            "display_modes": ["diagnostic_frame"],
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
    let script = r#"printf '%s\n' '{"schema":"elastos.browser.engine.supervisor-result/v1","page_id":"page:native-proof","adapter":"linux-chromium-headless","engine":"chromium_headless","stream_id":"stream:proof:test","network_mode":"runtime_net_only","direct_network":true,"wallet_injection":false,"display_session":{"schema":"elastos.browser.display-session/v1","session_id":"display:stream:proof:test","mode":"diagnostic_frame","network_mode":"runtime_net_only","direct_network":false,"input":"runtime_route","audio":false,"video":false}}'"#;
    let init = provider.init(json!({
        "adapters": [{
            "id": "linux-chromium-headless",
            "kind": "chromium_headless",
            "display_modes": ["diagnostic_frame"],
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
