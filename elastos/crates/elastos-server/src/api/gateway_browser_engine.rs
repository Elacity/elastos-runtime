//! Browser engine and Net/Exit summary helpers.

use super::*;

pub(in crate::api::gateway) async fn browser_engine_summary(
    registry: Option<&Arc<ProviderRegistry>>,
    principal_id: &str,
) -> serde_json::Value {
    let unavailable = || {
        serde_json::json!({
            "status": "unavailable",
            "provider": "elastos://browser-engine/*",
            "mode": "not_configured",
            "required": "native/webview or microVM Browser Engine Adapter",
            "stream_session_schema": "elastos.exit.stream-session/v1",
            "display_session_schema": "elastos.browser.display-session/v1",
            "supported_display_modes": [],
            "supported_guarantee_levels": [],
            "byte_transport": "not_attached",
            "direct_network": false,
            "wallet_injection": false,
            "reason": "Browser Engine Adapter is not installed. This surface can request Runtime networking, but it cannot render arbitrary sites inside ElastOS yet."
        })
    };
    let Some(registry) = registry else {
        return unavailable();
    };
    match registry
        .send_raw(
            "browser-engine",
            &serde_json::json!({
                "op": "status",
                "principal_id": principal_id,
            }),
        )
        .await
    {
        Ok(value) if value.get("status").and_then(|entry| entry.as_str()) == Some("ok") => {
            let data = provider_response_data(&value).unwrap_or(value);
            if authority_false_proof_missing(&data, "direct_network") {
                return invalid_provider_summary(
                    "elastos://browser-engine/*",
                    "Browser Engine Adapter status omitted direct_network=false proof.",
                );
            }
            if authority_false_proof_missing(&data, "wallet_injection") {
                return invalid_provider_summary(
                    "elastos://browser-engine/*",
                    "Browser Engine Adapter status omitted wallet_injection=false proof.",
                );
            }
            serde_json::json!({
                "status": data.get("status").cloned().unwrap_or_else(|| serde_json::json!("unavailable")),
                "provider": "elastos://browser-engine/*",
                "mode": data.get("provider").cloned().unwrap_or_else(|| serde_json::json!("browser-engine-adapter")),
                "operations": data.get("operations").cloned().unwrap_or_else(|| serde_json::json!(["status", "launch", "attach_stream", "close_page"])),
                "adapter_count": data.get("adapter_count").cloned().unwrap_or_else(|| serde_json::json!(0)),
                "adapters": browser_visible_engine_adapters(&data),
                "active_sessions": data.get("active_sessions").cloned().unwrap_or_else(|| serde_json::json!(0)),
                "max_active_sessions": data.get("max_active_sessions").cloned().unwrap_or_else(|| serde_json::json!(0)),
                "capacity_available": data.get("capacity_available").cloned().unwrap_or(serde_json::Value::Bool(false)),
                "stream_session_schema": data.get("stream_session_schema").cloned().unwrap_or_else(|| serde_json::json!("elastos.exit.stream-session/v1")),
                "display_session_schema": data.get("display_session_schema").cloned().unwrap_or_else(|| serde_json::json!("elastos.browser.display-session/v1")),
                "supported_display_modes": data.get("supported_display_modes").cloned().unwrap_or_else(|| serde_json::json!([])),
                "supported_guarantee_levels": data.get("supported_guarantee_levels").cloned().unwrap_or_else(|| serde_json::json!([])),
                "byte_transport": data.get("required_byte_transport").cloned().unwrap_or_else(|| serde_json::json!("adapter_ipc")),
                "direct_network": false,
                "wallet_injection": false,
                "reason": if data.get("status").and_then(|entry| entry.as_str()) == Some("configured") {
                    serde_json::json!("Browser Engine Adapter contract is configured; page rendering still requires attached stream byte transport.")
                } else {
                    serde_json::json!("Browser Engine Adapter provider is installed but no engine adapter is configured.")
                }
            })
        }
        _ => unavailable(),
    }
}

fn browser_visible_engine_adapters(data: &serde_json::Value) -> serde_json::Value {
    let Some(adapters) = data.get("adapters").and_then(|value| value.as_array()) else {
        return serde_json::json!([]);
    };
    serde_json::Value::Array(
        adapters
            .iter()
            .filter_map(|adapter| {
                let id = adapter.get("id").and_then(|value| value.as_str())?;
                if !is_safe_runtime_id(id) {
                    return None;
                }
                Some(serde_json::json!({
                    "id": id,
                    "engine": adapter.get("engine").and_then(|value| value.as_str()).unwrap_or("browser_engine"),
                    "default": adapter.get("default").and_then(|value| value.as_bool()).unwrap_or(false),
                    "backing_substrate": visible_runtime_string(adapter.get("backing_substrate")).unwrap_or("configured_provider"),
                    "supported_display_modes": visible_string_array(adapter.get("supported_display_modes")),
                    "supported_guarantee_levels": visible_string_array(adapter.get("supported_guarantee_levels")),
                    "network_mode": "runtime_net_only",
                    "direct_network": false,
                    "wallet_injection": false,
                }))
            })
            .collect(),
    )
}

fn visible_runtime_string(value: Option<&serde_json::Value>) -> Option<&str> {
    value
        .and_then(|value| value.as_str())
        .filter(|value| is_safe_runtime_id(value))
}

fn visible_string_array(value: Option<&serde_json::Value>) -> serde_json::Value {
    serde_json::Value::Array(
        value
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str())
                    .filter(|value| is_safe_runtime_id(value))
                    .map(|value| serde_json::Value::String(value.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

pub(in crate::api::gateway) async fn browser_net_summary(
    registry: Option<&Arc<ProviderRegistry>>,
    principal_id: &str,
) -> serde_json::Value {
    let exit_provider = browser_exit_summary(registry, principal_id).await;
    let Some(registry) = registry else {
        return serde_json::json!({
            "status": "fail_closed",
            "provider": "elastos://net/*",
            "operations": ["resolve", "connect", "stream", "http"],
            "direct_network": false,
            "exit_provider": exit_provider,
            "reason": "No Runtime Net provider is configured for this browser capsule."
        });
    };
    match registry
        .send_raw(
            "net",
            &serde_json::json!({
                "op": "status",
                "principal_id": principal_id,
            }),
        )
        .await
    {
        Ok(value) if value.get("status").and_then(|entry| entry.as_str()) == Some("ok") => {
            let data = provider_response_data(&value).unwrap_or(value);
            if authority_false_proof_missing(&data, "direct_network") {
                return serde_json::json!({
                    "status": "invalid_provider_status",
                    "provider": "elastos://net/*",
                    "operations": ["resolve", "connect", "stream", "http"],
                    "direct_network": false,
                    "exit_provider": exit_provider,
                    "reason": "Runtime Net provider status omitted direct_network=false proof."
                });
            }
            serde_json::json!({
                "status": data.get("status").cloned().unwrap_or_else(|| serde_json::json!("fail_closed")),
                "provider": "elastos://net/*",
                "operations": data.get("operations").cloned().unwrap_or_else(|| serde_json::json!(["resolve", "connect", "stream", "http"])),
                "direct_network": false,
                "exit_count": data.get("exit_count").cloned().unwrap_or_else(|| serde_json::json!(0)),
                "exit_provider": exit_provider,
                "reason": "Runtime Net provider is installed; egress remains unavailable until an Exit Provider backend is configured."
            })
        }
        Ok(value) => serde_json::json!({
            "status": "fail_closed",
            "provider": "elastos://net/*",
            "operations": ["resolve", "connect", "stream", "http"],
            "direct_network": false,
            "exit_provider": exit_provider,
            "reason": value.get("message").and_then(|entry| entry.as_str()).unwrap_or("Runtime Net provider returned an error.")
        }),
        Err(err) => serde_json::json!({
            "status": "fail_closed",
            "provider": "elastos://net/*",
            "operations": ["resolve", "connect", "stream", "http"],
            "direct_network": false,
            "exit_provider": exit_provider,
            "reason": format!("Runtime Net provider unavailable: {err}")
        }),
    }
}

async fn browser_exit_summary(
    registry: Option<&Arc<ProviderRegistry>>,
    principal_id: &str,
) -> serde_json::Value {
    let Some(registry) = registry else {
        return serde_json::Value::Null;
    };
    match registry
        .send_raw(
            "exit",
            &serde_json::json!({
                "op": "status",
                "principal_id": principal_id,
            }),
        )
        .await
    {
        Ok(value) if value.get("status").and_then(|entry| entry.as_str()) == Some("ok") => {
            let data = provider_response_data(&value).unwrap_or(value);
            if authority_false_proof_missing(&data, "direct_network") {
                return serde_json::json!({
                    "status": "invalid_provider_status",
                    "provider": "elastos://exit/*",
                    "operations": ["quote", "open_stream", "close_stream", "http_fetch"],
                    "direct_network": false,
                    "backend_count": 0,
                    "reason": "Browser Exit provider status omitted direct_network=false proof."
                });
            }
            serde_json::json!({
                "status": data.get("status").cloned().unwrap_or_else(|| serde_json::json!("fail_closed")),
                "provider": "elastos://exit/*",
                "operations": data.get("operations").cloned().unwrap_or_else(|| serde_json::json!(["quote", "open_stream", "close_stream", "http_fetch"])),
                "direct_network": false,
                "backend_count": data.get("backend_count").cloned().unwrap_or_else(|| serde_json::json!(0)),
                "remote_carrier_exit_count": data.get("remote_carrier_exit_count").cloned().unwrap_or_else(|| serde_json::json!(0)),
                "remote_carrier_exits": browser_visible_remote_carrier_exits(&data),
            })
        }
        _ => serde_json::Value::Null,
    }
}

fn browser_visible_remote_carrier_exits(data: &serde_json::Value) -> serde_json::Value {
    let Some(exits) = data
        .get("remote_carrier_exits")
        .and_then(|value| value.as_array())
    else {
        return serde_json::json!([]);
    };
    serde_json::Value::Array(
        exits
            .iter()
            .map(|exit| {
                let mut visible = exit.clone();
                scrub_exit_authority_fields(&mut visible);
                visible
            })
            .collect(),
    )
}

fn scrub_exit_authority_fields(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for key in [
                "allowed_principals",
                "adapter_ipc",
                "relay_ipc",
                "raw_socket",
                "connect_ticket",
            ] {
                object.remove(key);
            }
            for value in object.values_mut() {
                scrub_exit_authority_fields(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                scrub_exit_authority_fields(value);
            }
        }
        _ => {}
    }
}

fn authority_false_proof_missing(data: &serde_json::Value, field: &str) -> bool {
    data.get(field).and_then(|value| value.as_bool()) != Some(false)
}

fn invalid_provider_summary(provider: &str, reason: &str) -> serde_json::Value {
    serde_json::json!({
        "status": "invalid_provider_status",
        "provider": provider,
        "mode": "invalid_provider_status",
        "stream_session_schema": "elastos.exit.stream-session/v1",
        "display_session_schema": "elastos.browser.display-session/v1",
        "supported_display_modes": [],
        "supported_guarantee_levels": [],
        "byte_transport": "not_attached",
        "direct_network": false,
        "wallet_injection": false,
        "reason": reason,
    })
}
