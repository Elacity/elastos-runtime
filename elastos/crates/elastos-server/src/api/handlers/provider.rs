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

use elastos_common::localhost::rooted_localhost_uri;
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

/// Build the capability resource string for a provider proxy request.
///
/// First-party `elastos://` sub-providers (ai, did, peer) use `elastos://<scheme>/...`.
/// All other schemes use their native `<scheme>://*` format.
fn build_capability_resource(scheme: &str, op: &str, request: &Value) -> Result<String, String> {
    match scheme {
        "localhost" => match request
            .get("path")
            .and_then(|value| value.as_str())
            .filter(|path| !path.is_empty())
        {
            Some(path) => rooted_localhost_uri(path)
                .ok_or_else(|| format!("Invalid rooted localhost path: {}", path)),
            None => Err("localhost provider request missing path".to_string()),
        },
        "ai" => {
            let backend = request.get("backend").and_then(|v| v.as_str());
            match backend {
                Some(b) => {
                    if !b
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    {
                        return Err(format!("Invalid backend name: {}", b));
                    }
                    Ok(format!("elastos://ai/{}/{}", b, op))
                }
                None => Ok(format!("elastos://ai/meta/{}", op)),
            }
        }
        "did" | "peer" => Ok(format!("elastos://{}/*", scheme)),
        _ => Ok(format!("{}://*", scheme)),
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
    use axum::Extension;
    use elastos_runtime::provider::ProviderRegistry;
    use elastos_runtime::session::SessionType;

    #[test]
    fn test_ai_resource_with_backend() {
        let request = serde_json::json!({"backend": "local", "op": "chat_completions"});
        let result = build_capability_resource("ai", "chat_completions", &request);
        assert_eq!(result.unwrap(), "elastos://ai/local/chat_completions");
    }

    #[test]
    fn test_ai_resource_without_backend() {
        let request = serde_json::json!({"op": "list_backends"});
        let result = build_capability_resource("ai", "list_backends", &request);
        assert_eq!(result.unwrap(), "elastos://ai/meta/list_backends");
    }

    #[test]
    fn test_ai_resource_invalid_backend() {
        let request = serde_json::json!({"backend": "bad/name", "op": "chat"});
        let result = build_capability_resource("ai", "chat", &request);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid backend name"));
    }

    #[test]
    fn test_non_ai_resource() {
        let request =
            serde_json::json!({"op": "read", "path": "Local/SharedByLocalUsersAndBots/PC2/demo"});
        let result = build_capability_resource("localhost", "read", &request);
        assert_eq!(
            result.unwrap(),
            "localhost://Local/SharedByLocalUsersAndBots/PC2/demo"
        );
    }

    #[test]
    fn test_first_party_sub_provider_resource() {
        let request = serde_json::json!({"op": "get_did"});
        assert_eq!(
            build_capability_resource("did", "get_did", &request).unwrap(),
            "elastos://did/*"
        );
        assert_eq!(
            build_capability_resource("peer", "connect", &request).unwrap(),
            "elastos://peer/*"
        );
    }

    #[test]
    fn test_localhost_resource_requires_path() {
        let request = serde_json::json!({"op": "read"});
        let result = build_capability_resource("localhost", "read", &request);
        assert!(result.is_err());
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
