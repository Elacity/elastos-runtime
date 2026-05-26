use super::gateway_browser::{
    browser_provider_resource_call, provider_response_data, provider_response_error_message,
};
use super::BROWSER_CAPSULE_ID;
use serde_json::json;

#[test]
fn test_provider_response_data_unwraps_nested_provider_envelopes() {
    let response = json!({
        "status": "ok",
        "data": {
            "status": "ok",
            "data": {
                "schema": "elastos.browser.webrtc-answer/v1",
                "type": "answer",
                "sdp": "v=0\r\n"
            }
        }
    });

    let data = provider_response_data(&response).unwrap();
    assert_eq!(data["schema"], "elastos.browser.webrtc-answer/v1");
    assert_eq!(data["type"], "answer");
}

#[test]
fn test_provider_response_error_message_unwraps_nested_provider_errors() {
    let response = json!({
        "status": "ok",
        "data": {
            "status": "error",
            "code": "engine_process_unavailable",
            "message": "browser page not found"
        }
    });

    let message = provider_response_error_message(&response).unwrap();
    assert!(message.contains("engine_process_unavailable"));
    assert!(message.contains("browser page not found"));
}

#[test]
fn test_browser_provider_resource_call_separates_carrier_call_from_effect_resource() {
    let call = browser_provider_resource_call(
        "wallet",
        "request_signature",
        "elastos://wallet/eip155:20/sign/transaction_intent".to_string(),
        json!({
            "principal_id": "person:local:alice",
            "account_id": "wallet:eip155:20:0x1111111111111111111111111111111111111111",
            "chain_namespace": "eip155:20",
            "intent": "transaction_intent",
            "capsule_id": BROWSER_CAPSULE_ID,
            "resource": "elastos://chain/esc-mainnet/broadcast_transaction",
            "reason": "Browser page requests eth_sendTransaction on esc-mainnet",
            "payload": {
                "schema": "elastos.chain.unsigned_transaction_intent/v1"
            }
        }),
    )
    .expect("wallet signing provider call should be carrier-shaped");

    assert_eq!(call.scheme, "wallet");
    assert_eq!(
        call.resource,
        "elastos://wallet/eip155:20/sign/transaction_intent"
    );
    assert_eq!(call.request["op"], "request_signature");
    assert_eq!(
        call.request["resource"],
        "elastos://chain/esc-mainnet/broadcast_transaction"
    );
}

#[test]
fn test_browser_provider_resource_call_covers_net_exit_and_engine_open_chain() {
    let net = browser_provider_resource_call(
        "net",
        "stream",
        "elastos://net/stream".to_string(),
        json!({
            "target": "tls://glidefinance.io:443",
            "principal_id": "person:local:alice",
            "reason": "open browser page"
        }),
    )
    .expect("net stream call should be resource-shaped");
    assert_eq!(net.resource, "elastos://net/stream");
    assert_eq!(net.request["op"], "stream");

    let exit = browser_provider_resource_call(
        "exit",
        "open_stream",
        "elastos://exit/open_stream".to_string(),
        json!({
            "target": "tls://glidefinance.io:443",
            "principal_id": "person:local:alice",
            "reason": "open browser page"
        }),
    )
    .expect("exit open_stream call should be resource-shaped");
    assert_eq!(exit.resource, "elastos://exit/open_stream");
    assert_eq!(exit.request["op"], "open_stream");

    let engine = browser_provider_resource_call(
        "browser-engine",
        "launch",
        "elastos://browser-engine/launch".to_string(),
        json!({
            "url": "https://glidefinance.io/",
            "stream_session": {
                "schema": "elastos.exit.stream-session/v1",
                "stream_id": "stream:test",
                "target": "tls://glidefinance.io:443"
            },
            "principal_id": "person:local:alice",
            "reason": "open browser page",
            "wallet": {},
            "display_mode": "webrtc_remote_display"
        }),
    )
    .expect("browser engine launch call should be resource-shaped");
    assert_eq!(engine.resource, "elastos://browser-engine/launch");
    assert_eq!(engine.request["op"], "launch");
}

#[test]
fn test_browser_provider_resource_call_covers_engine_page_operations() {
    let cases = [
        ("page_status", "elastos://browser-engine/page/status"),
        ("frame", "elastos://browser-engine/page/frame"),
        ("screenshot", "elastos://browser-engine/page/screenshot"),
        ("input", "elastos://browser-engine/page/input"),
        ("close_page", "elastos://browser-engine/close_page"),
        (
            "webrtc_signal",
            "elastos://browser-engine/page/webrtc_signal",
        ),
    ];

    for (operation, resource) in cases {
        let call = browser_provider_resource_call(
            "browser-engine",
            operation,
            resource.to_string(),
            json!({
                "page_id": "page:test",
                "principal_id": "person:local:alice"
            }),
        )
        .expect("browser engine page operation should be resource-shaped");
        assert_eq!(call.resource, resource);
        assert_eq!(call.request["op"], operation);
    }
}
