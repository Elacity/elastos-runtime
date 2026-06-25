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

use crate::provider_resource::{build_capability_resource, required_action_for};
use elastos_runtime::capability::{Action, CapabilityManager, CapabilityToken, ResourceId};
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

    // PRE-AUDIT #3: the action the OPERATION requires (not the token's own action).
    let required_action = required_action_for(&op);
    enforce_capability(&state, &session, &headers, &resource, required_action).await?;

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

/// Validate that the session has permission for this provider operation.
async fn enforce_capability(
    state: &ProviderProxyState,
    session: &Session,
    headers: &HeaderMap,
    resource: &str,
    required_action: Action,
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

    // PRE-AUDIT #3: enforce the action the OPERATION requires, not the token's own action. The
    // bridge no longer trusts capsules to self-check fine-grained actions — a Read-granted token on
    // a write/delete/admin operation fails closed here (WrongAction).
    // G-ID flip: validate against session.vm_id (canonical capsule identity), fail
    // closed when absent.
    let caller_id = session.vm_id.as_deref().ok_or_else(|| {
        (
            StatusCode::FORBIDDEN,
            "session has no capsule identity".to_string(),
        )
    })?;
    cap_mgr
        .validate(&token, caller_id, required_action, &resource_id, None)
        .await
        .map_err(|e| (StatusCode::FORBIDDEN, format!("Capability denied: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::Extension;
    use elastos_runtime::provider::ProviderRegistry;
    use elastos_runtime::session::SessionType;

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
            Path(("peer".to_string(), "gossip_join".to_string())),
            HeaderMap::new(),
            "{\"topic\":\"#general\"}".to_string(),
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
            .contains("no provider for scheme: peer"));
    }
}
