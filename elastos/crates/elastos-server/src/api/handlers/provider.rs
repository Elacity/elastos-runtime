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

use crate::provider_resource::{
    build_capability_resource, ensure_generic_wallet_capability, provider_operation_action,
};
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
    let required_action = provider_operation_action(&scheme, &op).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("Unsupported provider operation action mapping: {scheme}/{op}"),
        )
    })?;
    ensure_generic_wallet_capability(&resource, required_action)
        .map_err(|msg| (StatusCode::BAD_REQUEST, msg))?;
    if scheme == "wallet" {
        request = serde_json::json!({"op": "status"});
    }

    enforce_capability(&state, &session, &headers, &resource, required_action).await?;
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
    required_action: Action,
) -> Result<(), (StatusCode, String)> {
    let bridge_capsule_id = headers
        .get("X-Elastos-Capsule-Id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty());
    let token_b64 = headers
        .get("X-Capability-Token")
        .and_then(|v| v.to_str().ok());

    // Shell sessions have orchestrator privilege for direct shell calls. Bridge
    // metadata makes this a delegated capsule call and therefore requires a
    // capability token, but it never replaces the authenticated session as the
    // token subject.
    if session.is_shell() && bridge_capsule_id.is_none() && token_b64.is_none() {
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

    let token_b64 = token_b64.ok_or_else(|| {
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
            required_action,
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
    use elastos_runtime::capability::{CapabilityManager, CapabilityStore, TokenConstraints};
    use elastos_runtime::primitives::{audit::AuditLog, metrics::MetricsManager};
    use elastos_runtime::provider::{
        Provider, ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse,
    };
    use elastos_runtime::session::SessionType;
    use tokio::sync::Mutex;

    #[derive(Default)]
    struct CapturingWalletProvider {
        requests: Mutex<Vec<Value>>,
    }

    #[async_trait::async_trait]
    impl Provider for CapturingWalletProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "capturing Wallet provider only supports raw status".to_string(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec!["wallet"]
        }

        fn name(&self) -> &'static str {
            "capturing-wallet"
        }

        async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
            self.requests.lock().await.push(request.clone());
            Ok(serde_json::json!({
                "status": "ok",
                "data": {
                    "provider": "wallet-provider",
                    "version": "0.2.0"
                }
            }))
        }
    }

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

    #[tokio::test]
    async fn generic_http_wallet_status_is_bounded_and_non_principal_specific() {
        let registry = Arc::new(ProviderRegistry::new());
        let provider = Arc::new(CapturingWalletProvider::default());
        registry
            .register_sub_provider("wallet", provider.clone())
            .await
            .unwrap();
        let state = ProviderProxyState {
            registry,
            capability_manager: None,
        };

        let response = provider_proxy(
            State(state),
            Extension(Session::new(SessionType::Shell, None)),
            Path(("wallet".to_string(), "status".to_string())),
            HeaderMap::new(),
            serde_json::json!({
                "principal_id": "caller-selected-principal",
                "token": "caller-selected-token",
                "request": {
                    "op": "wallet_contract"
                }
            })
            .to_string(),
        )
        .await
        .expect("read-only Wallet status should remain available");

        assert_eq!(response.0["status"], "ok");
        assert_eq!(
            *provider.requests.lock().await,
            vec![serde_json::json!({"op": "status"})]
        );
    }

    #[tokio::test]
    async fn generic_http_wallet_operations_fail_before_provider_invocation() {
        let registry = Arc::new(ProviderRegistry::new());
        let provider = Arc::new(CapturingWalletProvider::default());
        registry
            .register_sub_provider("wallet", provider.clone())
            .await
            .unwrap();
        let state = ProviderProxyState {
            registry,
            capability_manager: None,
        };

        for operation in [
            "wallet_contract",
            "challenge",
            "accounts",
            "request_signature",
            "export_managed_secret",
            "approval_requests",
            "broadcast_transaction",
        ] {
            let error = provider_proxy(
                State(state.clone()),
                Extension(Session::new(SessionType::Shell, None)),
                Path(("wallet".to_string(), operation.to_string())),
                HeaderMap::new(),
                serde_json::json!({
                    "principal_id": "caller-selected-principal",
                    "token": "caller-selected-token"
                })
                .to_string(),
            )
            .await
            .expect_err("generic Wallet authority must fail closed");

            assert_eq!(error.0, StatusCode::BAD_REQUEST, "{operation}");
        }

        assert!(
            provider.requests.lock().await.is_empty(),
            "rejected generic Wallet operations must not reach ProviderRegistry"
        );
    }

    #[tokio::test]
    async fn provider_proxy_validates_capsule_bridge_token_even_for_shell_bearer() {
        let audit_log = Arc::new(AuditLog::new());
        let capability_manager = Arc::new(CapabilityManager::new(
            Arc::new(CapabilityStore::new()),
            audit_log.clone(),
            Arc::new(MetricsManager::new()),
        ));
        let state = ProviderProxyState {
            registry: Arc::new(ProviderRegistry::new()),
            capability_manager: Some(capability_manager.clone()),
        };
        let session = Session::new(SessionType::Shell, None);
        let resource = "localhost://Local/SharedByLocalUsersAndBots/Home/a.md";
        let token = capability_manager.grant(
            session.id.as_str(),
            ResourceId::new(resource),
            Action::Read,
            TokenConstraints::default(),
            None,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Elastos-Capsule-Id",
            HeaderValue::from_static("component-test"),
        );
        headers.insert(
            "X-Capability-Token",
            HeaderValue::from_str(&token.to_base64().unwrap()).unwrap(),
        );

        enforce_capability(&state, &session, &headers, resource, Action::Read)
            .await
            .expect("bridge token should validate against authenticated session");

        assert!(audit_log.recent_events(10).iter().any(|event| matches!(
            event,
            elastos_runtime::primitives::audit::AuditEvent::CapabilityUse {
                capsule_id,
                success: true,
                ..
            } if capsule_id == session.id.as_str()
        )));
    }

    #[tokio::test]
    async fn provider_proxy_rejects_token_bound_only_to_claimed_capsule_id() {
        let capability_manager = Arc::new(CapabilityManager::new(
            Arc::new(CapabilityStore::new()),
            Arc::new(AuditLog::new()),
            Arc::new(MetricsManager::new()),
        ));
        let state = ProviderProxyState {
            registry: Arc::new(ProviderRegistry::new()),
            capability_manager: Some(capability_manager.clone()),
        };
        let session = Session::new(SessionType::Shell, None);
        let resource = "localhost://Local/SharedByLocalUsersAndBots/Home/a.md";
        let token = capability_manager.grant(
            "component-test",
            ResourceId::new(resource),
            Action::Read,
            TokenConstraints::default(),
            None,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Elastos-Capsule-Id",
            HeaderValue::from_static("component-test"),
        );
        headers.insert(
            "X-Capability-Token",
            HeaderValue::from_str(&token.to_base64().unwrap()).unwrap(),
        );

        let err = enforce_capability(&state, &session, &headers, resource, Action::Read)
            .await
            .expect_err("capsule metadata must not replace authenticated authority");

        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn provider_proxy_rejects_wrong_action_bridge_token_before_dispatch() {
        let capability_manager = Arc::new(CapabilityManager::new(
            Arc::new(CapabilityStore::new()),
            Arc::new(AuditLog::new()),
            Arc::new(MetricsManager::new()),
        ));
        let state = ProviderProxyState {
            registry: Arc::new(ProviderRegistry::new()),
            capability_manager: Some(capability_manager.clone()),
        };
        let session = Session::new(SessionType::Shell, None);
        let resource = "localhost://Local/SharedByLocalUsersAndBots/Home/a.md";
        let token = capability_manager.grant(
            session.id.as_str(),
            ResourceId::new(resource),
            Action::Write,
            TokenConstraints::default(),
            None,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Elastos-Capsule-Id",
            HeaderValue::from_static("component-test"),
        );
        headers.insert(
            "X-Capability-Token",
            HeaderValue::from_str(&token.to_base64().unwrap()).unwrap(),
        );

        let err = enforce_capability(&state, &session, &headers, resource, Action::Read)
            .await
            .expect_err("write token must not authorize read operation");

        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert!(err.1.contains("Capability denied"));
    }

    #[tokio::test]
    async fn provider_proxy_enforces_model_capability_on_session_path() {
        let audit_log = Arc::new(AuditLog::new());
        let capability_manager = Arc::new(CapabilityManager::new(
            Arc::new(CapabilityStore::new()),
            audit_log.clone(),
            Arc::new(MetricsManager::new()),
        ));
        let state = ProviderProxyState {
            registry: Arc::new(ProviderRegistry::new()),
            capability_manager: Some(capability_manager.clone()),
        };
        let session = Session::new(SessionType::Shell, None);

        // The resource + action the proxy derives for model/runs_create.
        let request = serde_json::json!({"offer_id": "offer:chat", "op": "runs_create"});
        let resource = build_capability_resource("model", "runs_create", &request).unwrap();
        assert_eq!(resource, "elastos://model/offer:chat/runs_create");
        let required_action = provider_operation_action("model", "runs_create").unwrap();
        assert_eq!(required_action, Action::Execute);

        // Delegated capsule call without a token: denied, no ambient authority.
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Elastos-Capsule-Id",
            HeaderValue::from_static("component-test"),
        );
        let err = enforce_capability(&state, &session, &headers, &resource, required_action)
            .await
            .expect_err("model runs_create without a token must be denied");
        assert_eq!(err.0, StatusCode::FORBIDDEN);

        // Read-scoped token must not authorize an Execute operation.
        let read_token = capability_manager.grant(
            session.id.as_str(),
            ResourceId::new(&resource),
            Action::Read,
            TokenConstraints::default(),
            None,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Elastos-Capsule-Id",
            HeaderValue::from_static("component-test"),
        );
        headers.insert(
            "X-Capability-Token",
            HeaderValue::from_str(&read_token.to_base64().unwrap()).unwrap(),
        );
        let err = enforce_capability(&state, &session, &headers, &resource, required_action)
            .await
            .expect_err("read token must not authorize model runs_create");
        assert_eq!(err.0, StatusCode::FORBIDDEN);

        // Execute token on the derived resource validates.
        let execute_token = capability_manager.grant(
            session.id.as_str(),
            ResourceId::new(&resource),
            Action::Execute,
            TokenConstraints::default(),
            None,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Elastos-Capsule-Id",
            HeaderValue::from_static("component-test"),
        );
        headers.insert(
            "X-Capability-Token",
            HeaderValue::from_str(&execute_token.to_base64().unwrap()).unwrap(),
        );
        enforce_capability(&state, &session, &headers, &resource, required_action)
            .await
            .expect("execute token should authorize model runs_create");
    }
}
