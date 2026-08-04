use super::gateway_browser::{
    browser_provider_resource_call, browser_terminal_close_receipt, provider_response_data,
    provider_response_error_message, BrowserEngineCleanup,
};
use crate::api::browser_engine_protocol::{
    BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA, BROWSER_ENGINE_CLEANUP_RESULT_SCHEMA,
    BROWSER_ENGINE_PROTOCOL_VERSION,
};
use serde_json::json;

fn test_browser_engine_cleanup(page_id: &str) -> BrowserEngineCleanup {
    BrowserEngineCleanup {
        cleanup_id: "browser-cleanup:test".to_string(),
        page_id: page_id.to_string(),
        principal_id: "person:local:test".to_string(),
        owner_launch_id: "launch:test".to_string(),
        browser_instance: None,
        generation: "sha256:test-generation".to_string(),
        engine_route_provider: "mock-browser-route".to_string(),
        engine_provider: "browser-engine-adapter".to_string(),
        engine_protocol_version: BROWSER_ENGINE_PROTOCOL_VERSION.to_string(),
        engine_adapter: "mock-adapter".to_string(),
        engine: "mock-engine".to_string(),
        stream_id: "stream:test".to_string(),
        transport_authority: None,
        provider_cleanup: json!({
            "schema": BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA,
            page_id: page_id.to_string(),
            "generation": "sha256:test-generation",
            "adapter": "mock-adapter",
            "engine": "mock-engine",
            "stream_id": "stream:test",
        }),
    }
}

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
fn test_browser_close_requires_exact_typed_terminal_cleanup_proof() {
    let cleanup = test_browser_engine_cleanup("page:typed-terminal");
    let receipt = json!({
        "schema": BROWSER_ENGINE_CLEANUP_RESULT_SCHEMA,
        "page_id": cleanup.page_id,
        "generation": cleanup.generation,
        "binding": cleanup.provider_cleanup,
        "terminal": true,
        "effects": {
            "page_absent": true,
            "child_absent": true,
            "vm_absent": true,
            "route_absent": true,
            "socket_absent": true,
        }
    });
    assert!(browser_terminal_close_receipt(&cleanup, receipt).is_ok());
}

#[test]
fn test_browser_close_rejects_absent_or_substituted_terminal_proof() {
    let cleanup = test_browser_engine_cleanup("page:still-owned");
    for receipt in [
        json!({
            "schema": BROWSER_ENGINE_CLEANUP_RESULT_SCHEMA,
            "page_id": cleanup.page_id,
            "generation": cleanup.generation,
            "binding": cleanup.provider_cleanup,
            "terminal": false,
            "effects": {}
        }),
        json!({
            "schema": BROWSER_ENGINE_CLEANUP_RESULT_SCHEMA,
            "page_id": cleanup.page_id,
            "generation": "sha256:substituted",
            "binding": cleanup.provider_cleanup,
            "terminal": true,
            "effects": {
                "page_absent": true,
                "child_absent": true,
                "vm_absent": true,
                "route_absent": true,
                "socket_absent": true,
            }
        }),
    ] {
        assert!(browser_terminal_close_receipt(&cleanup, receipt).is_err());
    }
}

#[test]
fn test_browser_provider_resource_call_rejects_generic_wallet_signing() {
    let error = browser_provider_resource_call(
        "wallet",
        "request_signature",
        "elastos://wallet/eip155:20/sign/transaction_intent".to_string(),
        json!({
            "account_id": "wallet:eip155:20:0x1111111111111111111111111111111111111111",
            "chain_namespace": "eip155:20",
            "intent": "transaction_intent",
            "resource": "elastos://chain/esc-mainnet/broadcast_transaction",
            "reason": "Browser page requests eth_sendTransaction on esc-mainnet",
            "payload": {
                "schema": "elastos.chain.unsigned_transaction_intent/v1"
            }
        }),
    )
    .err()
    .expect("Browser signing must use the private typed Wallet adapter");

    assert_eq!(error.0, axum::http::StatusCode::BAD_REQUEST);
    assert!(error
        .1
        .contains("Unsupported wallet provider operation: request_signature"));
}

#[test]
fn test_browser_provider_resource_call_rejects_predeclared_runtime_metadata() {
    for reserved in [
        "_runtime_invocation",
        "_runtime_transfer",
        "connect_ticket",
        "carrier_route",
        "carrier",
    ] {
        let err = match browser_provider_resource_call(
            "browser-engine",
            "page_status",
            "elastos://browser-engine/page/status".to_string(),
            json!({
                "page_id": "page:test",
                reserved: { "schema": "spoofed" }
            }),
        ) {
            Ok(_) => panic!("Browser provider call should reject Runtime-owned metadata"),
            Err(err) => err,
        };
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(err.1.contains("must not predeclare Runtime metadata field"));
        assert!(err.1.contains(reserved));
    }
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
            "profile": {
                "schema": "elastos.browser.profile/v1",
                "scope": "active_principal",
                "storage": "principal_owned_profile_disk",
                "storage_posture": "principal_owned_reset_scoped_unprotected",
                "protected_storage": false,
                "encrypted": false,
                "recoverable": false,
                "recovery": "not_recovery_kit_packaged",
                "uri": "localhost://Users/0123456789ab/BrowserProfiles/default/profile.ext4",
                "public_uri": "localhost://Users/self/BrowserProfiles/default/profile.ext4",
                "profile_key": "profile-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "disk_path": "/tmp/elastos-browser-profile-test/BrowserProfiles/default/profile.ext4",
                "reset": "whole_profile"
            },
            "principal_id": "person:local:alice",
            "reason": "open browser page",
            "wallet": {},
            "display_mode": "webrtc_remote_display",
            "guarantee_level": "operator_rbi"
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
        ("diagnostics", "elastos://browser-engine/page/diagnostics"),
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
