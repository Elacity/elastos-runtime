//! Generic provider proxy handler
//!
//! POST /api/provider/:scheme/:op
//! Routes arbitrary JSON to any registered provider capsule.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Extension, Json,
};
use serde_json::Value;

use crate::provider_resource::build_capability_resource;
use elastos_runtime::capability::{CapabilityManager, CapabilityToken, ResourceId};
use elastos_runtime::provider::ProviderRegistry;
use elastos_runtime::session::Session;

/// Shared state for the provider proxy handler
#[derive(Clone)]
pub struct ProviderProxyState {
    pub registry: Arc<ProviderRegistry>,
    pub capability_manager: Option<Arc<CapabilityManager>>,
}

/// POST /api/provider/:scheme/:op
///
/// Generic proxy — validates capability, forwards JSON to provider, returns response.
/// The `op` from the URL path is merged into the JSON body.
pub async fn provider_proxy(
    State(state): State<ProviderProxyState>,
    Extension(session): Extension<Session>,
    Path((scheme, op)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<Value>, (StatusCode, String)> {
    // Build request JSON first (need body for AI resource construction)
    let mut request: Value = if body.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&body)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid JSON body: {}", e)))?
    };
    request["op"] = Value::String(op.clone());

    // Build capability resource
    let resource = build_capability_resource(&scheme, &op, &request)
        .map_err(|msg| (StatusCode::BAD_REQUEST, msg))?;

    enforce_capability(&state, &session, &headers, &resource).await?;
    attach_localhost_provider_wire_token(&scheme, &op, &headers, &mut request);

    // Forward to provider
    let response = state.registry.send_raw(&scheme, &request).await;

    let response = match response {
        Ok(value) => value,
        Err(e) => {
            return Ok(Json(serde_json::json!({
                "status": "error",
                "code": "provider_error",
                "message": e.to_string(),
            })));
        }
    };

    Ok(Json(response))
}

fn attach_localhost_provider_wire_token(
    scheme: &str,
    op: &str,
    headers: &HeaderMap,
    request: &mut Value,
) {
    if scheme != "localhost"
        || !matches!(
            op,
            "read" | "write" | "list" | "delete" | "stat" | "mkdir" | "exists"
        )
        || request.get("token").is_some()
    {
        return;
    }
    let Some(token) = headers
        .get("X-Capability-Token")
        .and_then(|value| value.to_str().ok())
        .filter(|token| !token.is_empty())
    else {
        return;
    };
    if let Some(object) = request.as_object_mut() {
        object.insert("token".to_string(), Value::String(token.to_string()));
    }
}

/// Validate that the session has permission for this provider operation.
async fn enforce_capability(
    state: &ProviderProxyState,
    session: &Session,
    headers: &HeaderMap,
    resource: &str,
) -> Result<(), (StatusCode, String)> {
    // Shell sessions have orchestrator privilege
    if session.is_shell() {
        return Ok(());
    }

    let cap_mgr = match state.capability_manager {
        Some(ref mgr) => mgr,
        None => {
            return Err((
                StatusCode::FORBIDDEN,
                "Capability manager not configured — access denied (no ambient authority)"
                    .to_string(),
            ));
        }
    };

    let token_b64 = headers
        .get("X-Capability-Token")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                "Missing X-Capability-Token header".to_string(),
            )
        })?;

    let token = CapabilityToken::from_base64(token_b64).map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            format!("Invalid capability token: {}", e),
        )
    })?;

    let resource_id = ResourceId::new(resource);

    // Use the token's own action — the shell granted it for this purpose.
    // The provider capsule enforces fine-grained action checks.
    cap_mgr
        .validate(
            &token,
            session.id.as_str(),
            token.action(),
            &resource_id,
            None,
        )
        .await
        .map_err(|e| (StatusCode::FORBIDDEN, format!("Capability denied: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::http::HeaderValue;
    use axum::Extension;
    use elastos_runtime::provider::ProviderRegistry;
    use elastos_runtime::session::SessionType;

    #[test]
    fn localhost_provider_proxy_attaches_validated_header_token_to_wire_body() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Capability-Token", HeaderValue::from_static("cap-token"));
        let mut request = serde_json::json!({
            "op": "read",
            "path": "localhost://Local/SharedByLocalUsersAndBots/Home/sessions/a/snapshot.json"
        });

        attach_localhost_provider_wire_token("localhost", "read", &headers, &mut request);

        assert_eq!(
            request.get("token").and_then(|value| value.as_str()),
            Some("cap-token")
        );
    }

    #[test]
    fn provider_proxy_does_not_attach_header_token_to_other_providers() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Capability-Token", HeaderValue::from_static("cap-token"));
        let mut request = serde_json::json!({
            "op": "status"
        });

        attach_localhost_provider_wire_token("chain", "status", &headers, &mut request);

        assert!(request.get("token").is_none());
    }

    #[tokio::test]
    async fn test_provider_proxy_returns_structured_provider_error() {
        let state = ProviderProxyState {
            registry: Arc::new(ProviderRegistry::new()),
            capability_manager: None,
        };
        let session = Session::new(SessionType::Shell, None);

        let response = provider_proxy(
            State(state),
            Extension(session),
            Path(("chain".to_string(), "networks".to_string())),
            HeaderMap::new(),
            "{}".to_string(),
        )
        .await
        .expect("provider proxy should return structured JSON");

        let body = response.0;
        assert_eq!(body.get("status").and_then(|v| v.as_str()), Some("error"));
        assert_eq!(
            body.get("code").and_then(|v| v.as_str()),
            Some("provider_error")
        );
        assert!(body
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .contains("no provider for scheme: chain"));
    }
}
