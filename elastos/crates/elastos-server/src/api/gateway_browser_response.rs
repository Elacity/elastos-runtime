//! Browser provider envelope and response helpers.

use super::*;
use crate::provider_resource::build_capability_resource;

pub(in crate::api::gateway) fn provider_response_data(
    response: &serde_json::Value,
) -> Option<serde_json::Value> {
    let mut current = response;
    for _ in 0..4 {
        if current.get("status").and_then(|value| value.as_str()) == Some("ok") {
            if let Some(data) = current.get("data").filter(|value| value.is_object()) {
                current = data;
                continue;
            }
        }
        return current.is_object().then(|| current.clone());
    }
    current.is_object().then(|| current.clone())
}

pub(in crate::api::gateway) fn browser_provider_resource_call(
    scheme: &'static str,
    operation: &'static str,
    expected_resource: String,
    mut request: serde_json::Value,
) -> Result<BrowserProviderResourceCall, (StatusCode, String)> {
    if let Some(field) = provider_proxy_runtime_metadata_field(&request) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("provider request must not predeclare Runtime metadata field {field}"),
        ));
    }
    let resource = build_capability_resource(scheme, operation, &request).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid Browser provider resource: {err}"),
        )
    })?;
    if resource != expected_resource {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "Browser provider resource mismatch for {scheme}/{operation}: expected {expected_resource}, got {resource}"
            ),
        ));
    }
    request["op"] = serde_json::Value::String(operation.to_string());
    Ok(BrowserProviderResourceCall {
        scheme,
        resource,
        request,
    })
}

#[derive(Debug)]
pub(in crate::api::gateway) struct BrowserProviderResourceFailure {
    pub(in crate::api::gateway) status: StatusCode,
    pub(in crate::api::gateway) diagnostic: anyhow::Error,
    response_message: String,
}

pub(in crate::api::gateway) async fn browser_provider_resource_response(
    state: &GatewayState,
    call: BrowserProviderResourceCall,
) -> Result<serde_json::Value, (StatusCode, String)> {
    browser_provider_resource_result(state, call)
        .await
        .map_err(|failure| (failure.status, failure.response_message))
}

// Keep the original cause until the operation chooses its public projection.
pub(in crate::api::gateway) async fn browser_provider_resource_result(
    state: &GatewayState,
    call: BrowserProviderResourceCall,
) -> Result<serde_json::Value, BrowserProviderResourceFailure> {
    let registry = state.provider_registry.as_ref().ok_or_else(|| {
        let response_message = format!("{} provider unavailable", call.scheme);
        BrowserProviderResourceFailure {
            status: StatusCode::SERVICE_UNAVAILABLE,
            diagnostic: anyhow::anyhow!(response_message.clone()),
            response_message,
        }
    })?;
    if call.scheme == "browser-engine"
        && (call.request.get("page_id").is_some()
            || call.request.get("lifecycle_generation").is_some())
    {
        match gateway_browser_remote::consumer_binding(&state.data_dir, &call.request) {
            Ok(Some(binding)) => {
                return gateway_browser_remote::dispatch_consumer(state, &binding, &call.request)
                    .await
                    .map_err(|diagnostic| {
                        let message =
                            "Remote Runtime Engine operation is unavailable or pending settlement";
                        BrowserProviderResourceFailure {
                            status: StatusCode::SERVICE_UNAVAILABLE,
                            diagnostic: diagnostic.context(message),
                            response_message: message.into(),
                        }
                    })
            }
            Ok(None) => {}
            Err(diagnostic) => {
                let message = "Remote Runtime Engine ownership is unavailable";
                return Err(BrowserProviderResourceFailure {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    diagnostic: diagnostic.context(message),
                    response_message: message.into(),
                });
            }
        }
    }
    registry
        .send_raw(call.scheme, &call.request)
        .await
        .map_err(|diagnostic| BrowserProviderResourceFailure {
            status: StatusCode::SERVICE_UNAVAILABLE,
            response_message: format!(
                "{} provider unavailable for {}: {}",
                call.scheme, call.resource, diagnostic
            ),
            diagnostic: anyhow::anyhow!(diagnostic).context(format!(
                "{} provider unavailable for {}",
                call.scheme, call.resource
            )),
        })
}

pub(in crate::api::gateway) fn provider_response_error_message(
    response: &serde_json::Value,
) -> Option<String> {
    let mut current = response;
    for _ in 0..4 {
        match current.get("status").and_then(|value| value.as_str()) {
            Some("error") => {
                let code = current
                    .get("code")
                    .and_then(|value| value.as_str())
                    .unwrap_or("provider_error");
                let message = current
                    .get("message")
                    .and_then(|value| value.as_str())
                    .unwrap_or("provider returned an error");
                return Some(format!("{code}: {message}"));
            }
            Some("ok") => {
                let data = current.get("data").filter(|value| value.is_object())?;
                current = data;
            }
            _ => return None,
        }
    }
    None
}
