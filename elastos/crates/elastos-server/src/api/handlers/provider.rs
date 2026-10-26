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

use crate::auth::principal_localhost_root;
use crate::provider_resource::{
    build_capability_resource, ensure_generic_wallet_capability, provider_operation_action,
};
use crate::resource_bridge::scope_current_user_alias;
use elastos_common::localhost::rooted_localhost_uri;
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

    // The localhost provider stores every caller's path in one filesystem.
    // Bind Users homes to the authenticated session before any capability
    // check or provider dispatch. A shell session otherwise skips the token
    // check, and a `localhost://Users/*` grant matches every user directory.
    if scheme == "localhost" {
        scope_localhost_provider_request(&session, &mut request)?;
    }

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

    authorize_provider_request(&state, &session, &headers, &resource, required_action).await?;
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

fn session_principal_id(session: &Session) -> Option<&str> {
    session.owner.as_deref().filter(|owner| !owner.is_empty())
}

fn localhost_path_escapes(path: &str) -> bool {
    path.split(['/', '\\'])
        .any(|segment| segment == "." || segment == "..")
}

fn is_users_home(path: &str) -> bool {
    path == "localhost://Users" || path.starts_with("localhost://Users/")
}

fn scope_localhost_provider_request(
    session: &Session,
    request: &mut Value,
) -> Result<(), (StatusCode, String)> {
    let Some(path) = request
        .get("path")
        .and_then(|value| value.as_str())
        .filter(|path| !path.is_empty())
    else {
        return Ok(());
    };
    if localhost_path_escapes(path) {
        return Err((
            StatusCode::FORBIDDEN,
            "localhost path escapes the caller space".to_string(),
        ));
    }
    let Some(rooted) = rooted_localhost_uri(path) else {
        return Ok(());
    };
    if !is_users_home(&rooted) {
        return Ok(());
    }

    let principal_id = session_principal_id(session);
    let scoped = scope_current_user_alias(&rooted, principal_id)
        .map_err(|message| (StatusCode::FORBIDDEN, message))?;
    if localhost_path_escapes(&scoped) {
        return Err((
            StatusCode::FORBIDDEN,
            "localhost path escapes the caller space".to_string(),
        ));
    }
    let Some(principal_id) = principal_id else {
        return Err((
            StatusCode::FORBIDDEN,
            "localhost://Users requires a principal-scoped launch context".to_string(),
        ));
    };
    let root = principal_localhost_root(principal_id);
    if scoped != root && !scoped.starts_with(&format!("{root}/")) {
        return Err((
            StatusCode::FORBIDDEN,
            "localhost://Users roots must use Users/self or the active principal root".to_string(),
        ));
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("path".to_string(), Value::String(scoped));
    }
    Ok(())
}

fn caller_self_alias(session: &Session, resource: &str) -> Option<String> {
    let root = principal_localhost_root(session_principal_id(session)?);
    if resource == root {
        return Some("localhost://Users/self".to_string());
    }
    let rest = resource.strip_prefix(&format!("{root}/"))?;
    if rest.is_empty() || localhost_path_escapes(rest) {
        return None;
    }
    Some(format!("localhost://Users/self/{rest}"))
}

/// Accept the caller's concrete home and the `Users/self` alias of that same path.
///
/// Grants minted for `localhost://Users/self/...` stay valid after the proxy
/// rewrites the path into the session owner's directory.
async fn authorize_provider_request(
    state: &ProviderProxyState,
    session: &Session,
    headers: &HeaderMap,
    resource: &str,
    required_action: Action,
) -> Result<(), (StatusCode, String)> {
    if let Err(error) = enforce_capability(state, session, headers, resource, required_action).await
    {
        let Some(alias) = caller_self_alias(session, resource) else {
            return Err(error);
        };
        return enforce_capability(state, session, headers, &alias, required_action).await;
    }
    Ok(())
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

    // Shell sessions keep orchestrator privilege for direct Read calls.
    // Content and availability Write still require a capability token.
    if session.is_shell() && bridge_capsule_id.is_none() && token_b64.is_none() {
        if shell_write_requires_token(resource, required_action) {
            return Err((
                StatusCode::FORBIDDEN,
                "Missing X-Capability-Token header".to_string(),
            ));
        }
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

fn shell_write_requires_token(resource: &str, required_action: Action) -> bool {
    required_action == Action::Write
        && (resource.starts_with("elastos://content/")
            || resource.starts_with("elastos://availability/"))
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

    struct CapturingSchemeProvider {
        scheme: &'static str,
        requests: Mutex<Vec<Value>>,
    }

    impl CapturingSchemeProvider {
        fn new(scheme: &'static str) -> Self {
            Self {
                scheme,
                requests: Mutex::new(Vec::new()),
            }
        }
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

    #[async_trait::async_trait]
    impl Provider for CapturingSchemeProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "capturing scheme provider only supports raw requests".to_string(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec![self.scheme]
        }

        fn name(&self) -> &'static str {
            "capturing-scheme"
        }

        async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
            self.requests.lock().await.push(request.clone());
            Ok(serde_json::json!({ "status": "ok" }))
        }
    }

    async fn proxy_as_shell(
        scheme: &'static str,
        op: &str,
        headers: HeaderMap,
        body: &str,
    ) -> (
        Arc<CapturingSchemeProvider>,
        Result<Json<Value>, (StatusCode, String)>,
    ) {
        let registry = Arc::new(ProviderRegistry::new());
        let provider = Arc::new(CapturingSchemeProvider::new(scheme));
        registry
            .register_sub_provider(scheme, provider.clone())
            .await
            .unwrap();
        let state = ProviderProxyState {
            registry,
            capability_manager: None,
        };
        let result = provider_proxy(
            State(state),
            Extension(Session::new(SessionType::Shell, None)),
            Path((scheme.to_string(), op.to_string())),
            headers,
            body.to_string(),
        )
        .await;
        (provider, result)
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
    async fn shell_bearer_without_token_cannot_publish_or_unpublish_content() {
        for op in ["publish", "unpublish"] {
            let (provider, result) = proxy_as_shell("content", op, HeaderMap::new(), "{}").await;
            let err = result.expect_err(op);
            assert_eq!(err.0, StatusCode::FORBIDDEN, "{op}");
            assert!(provider.requests.lock().await.is_empty(), "{op}");
        }
    }

    #[tokio::test]
    async fn shell_bearer_without_token_cannot_ensure_availability() {
        let (provider, result) =
            proxy_as_shell("availability", "ensure", HeaderMap::new(), "{}").await;
        let err = result.expect_err("availability ensure");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert!(provider.requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn content_unpin_and_import_object_stay_unsupported() {
        for op in ["unpin", "import_object"] {
            let (provider, result) = proxy_as_shell("content", op, HeaderMap::new(), "{}").await;
            let err = result.expect_err(op);
            assert_eq!(err.0, StatusCode::BAD_REQUEST, "{op}");
            assert!(provider.requests.lock().await.is_empty(), "{op}");
        }
    }

    #[tokio::test]
    async fn content_fetch_with_fetch_token_still_dispatches() {
        let registry = Arc::new(ProviderRegistry::new());
        let provider = Arc::new(CapturingSchemeProvider::new("content"));
        registry
            .register_sub_provider("content", provider.clone())
            .await
            .unwrap();
        let capability_manager = Arc::new(CapabilityManager::new(
            Arc::new(CapabilityStore::new()),
            Arc::new(AuditLog::new()),
            Arc::new(MetricsManager::new()),
        ));
        let session = Session::new(SessionType::Shell, None);
        let token = capability_manager.grant(
            session.id.as_str(),
            ResourceId::new("elastos://content/fetch"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Capability-Token",
            HeaderValue::from_str(&token.to_base64().unwrap()).unwrap(),
        );
        let state = ProviderProxyState {
            registry,
            capability_manager: Some(capability_manager),
        };

        let response = provider_proxy(
            State(state),
            Extension(session),
            Path(("content".to_string(), "fetch".to_string())),
            headers,
            "{}".to_string(),
        )
        .await
        .expect("fetch-scoped token still authorizes content fetch");

        assert_eq!(response.0["status"], "ok");
        assert_eq!(
            *provider.requests.lock().await,
            vec![serde_json::json!({ "op": "fetch" })]
        );
    }

    #[tokio::test]
    async fn model_grant_does_not_authorize_content_publish() {
        let registry = Arc::new(ProviderRegistry::new());
        let provider = Arc::new(CapturingSchemeProvider::new("content"));
        registry
            .register_sub_provider("content", provider.clone())
            .await
            .unwrap();
        let capability_manager = Arc::new(CapabilityManager::new(
            Arc::new(CapabilityStore::new()),
            Arc::new(AuditLog::new()),
            Arc::new(MetricsManager::new()),
        ));
        let session = Session::new(SessionType::Shell, None);
        let token = capability_manager.grant(
            session.id.as_str(),
            ResourceId::new("elastos://model/offers_list"),
            Action::Read,
            TokenConstraints::default(),
            None,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Capability-Token",
            HeaderValue::from_str(&token.to_base64().unwrap()).unwrap(),
        );
        let state = ProviderProxyState {
            registry,
            capability_manager: Some(capability_manager),
        };

        let err = provider_proxy(
            State(state),
            Extension(session),
            Path(("content".to_string(), "publish".to_string())),
            headers,
            "{}".to_string(),
        )
        .await
        .expect_err("model grant must not satisfy content publish");

        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert!(provider.requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn content_status_without_token_still_dispatches() {
        let (provider, result) = proxy_as_shell("content", "status", HeaderMap::new(), "{}").await;
        let response = result.expect("shell Read still has ambient content status");
        assert_eq!(response.0["status"], "ok");
        assert_eq!(
            *provider.requests.lock().await,
            vec![serde_json::json!({ "op": "status" })]
        );
    }

    #[tokio::test]
    async fn content_publish_with_write_token_still_dispatches() {
        let registry = Arc::new(ProviderRegistry::new());
        let provider = Arc::new(CapturingSchemeProvider::new("content"));
        registry
            .register_sub_provider("content", provider.clone())
            .await
            .unwrap();
        let capability_manager = Arc::new(CapabilityManager::new(
            Arc::new(CapabilityStore::new()),
            Arc::new(AuditLog::new()),
            Arc::new(MetricsManager::new()),
        ));
        let session = Session::new(SessionType::Shell, None);
        let token = capability_manager.grant(
            session.id.as_str(),
            ResourceId::new("elastos://content/publish"),
            Action::Write,
            TokenConstraints::default(),
            None,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Capability-Token",
            HeaderValue::from_str(&token.to_base64().unwrap()).unwrap(),
        );
        let state = ProviderProxyState {
            registry,
            capability_manager: Some(capability_manager),
        };

        let response = provider_proxy(
            State(state),
            Extension(session),
            Path(("content".to_string(), "publish".to_string())),
            headers,
            "{}".to_string(),
        )
        .await
        .expect("write token still authorizes content publish");

        assert_eq!(response.0["status"], "ok");
        assert_eq!(
            *provider.requests.lock().await,
            vec![serde_json::json!({ "op": "publish" })]
        );
    }

    fn owned_session(owner: &str, session_type: SessionType) -> Session {
        Session::with_owner(session_type, None, owner.to_string())
    }

    fn capability_manager() -> Arc<CapabilityManager> {
        Arc::new(CapabilityManager::new(
            Arc::new(CapabilityStore::new()),
            Arc::new(AuditLog::new()),
            Arc::new(MetricsManager::new()),
        ))
    }

    fn token_headers(token: &elastos_runtime::capability::CapabilityToken) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Capability-Token",
            HeaderValue::from_str(&token.to_base64().unwrap()).unwrap(),
        );
        headers
    }

    async fn dispatch_localhost(
        session: Session,
        op: &str,
        body: Value,
        headers: HeaderMap,
        capability_manager: Option<Arc<CapabilityManager>>,
    ) -> (Vec<Value>, Result<Json<Value>, (StatusCode, String)>) {
        let registry = Arc::new(ProviderRegistry::new());
        let provider = Arc::new(CapturingSchemeProvider::new("localhost"));
        registry.register(provider.clone()).await;
        let state = ProviderProxyState {
            registry,
            capability_manager,
        };
        let result = provider_proxy(
            State(state),
            Extension(session),
            Path(("localhost".to_string(), op.to_string())),
            headers,
            body.to_string(),
        )
        .await;
        let requests = provider.requests.lock().await.clone();
        (requests, result)
    }

    fn assert_localhost_refused(
        op: &str,
        requests: &[Value],
        result: Result<Json<Value>, (StatusCode, String)>,
        message: &str,
    ) {
        let error = result.expect_err(op);
        assert_eq!(error.0, StatusCode::FORBIDDEN, "{op}: {}", error.1);
        assert!(
            error.1.contains(message),
            "{op}: {} should mention {message}",
            error.1
        );
        assert!(requests.is_empty(), "{op} reached the provider");
    }

    fn assert_localhost_scoped(
        op: &str,
        requests: &[Value],
        result: Result<Json<Value>, (StatusCode, String)>,
        expected_path: &str,
    ) {
        let response = result.expect(op);
        assert_eq!(response.0["status"], "ok", "{op}");
        assert_eq!(requests.len(), 1, "{op}");
        assert_eq!(requests[0]["op"], op);
        assert_eq!(requests[0]["path"], expected_path);
    }

    #[tokio::test]
    async fn user_a_cannot_read_or_write_user_b_localhost_paths() {
        let owner_a = "user-a";
        let owner_b = "user-b";
        let root_a = crate::auth::principal_localhost_root(owner_a);
        let root_b = crate::auth::principal_localhost_root(owner_b);
        assert_ne!(root_a, root_b);
        let foreign_file = format!("{root_b}/Documents/secret.txt");
        let manager = capability_manager();
        let foreign_message =
            "localhost://Users roots must use Users/self or the active principal root";

        for op in ["read", "write"] {
            let shell = owned_session(owner_a, SessionType::Shell);
            let body = match op {
                "write" => serde_json::json!({"path": foreign_file, "content": "from-a"}),
                _ => serde_json::json!({"path": foreign_file}),
            };
            let (requests, result) =
                dispatch_localhost(shell, op, body, HeaderMap::new(), Some(manager.clone())).await;
            assert_localhost_refused(op, &requests, result, foreign_message);

            let app = owned_session(owner_a, SessionType::Capsule);
            let action = if op == "write" {
                Action::Write
            } else {
                Action::Read
            };
            let token = manager.grant(
                app.id.as_str(),
                ResourceId::new("localhost://Users/*"),
                action,
                TokenConstraints::default(),
                None,
            );
            let body = match op {
                "write" => serde_json::json!({"path": foreign_file, "content": "from-a"}),
                _ => serde_json::json!({"path": foreign_file}),
            };
            let (requests, result) =
                dispatch_localhost(app, op, body, token_headers(&token), Some(manager.clone()))
                    .await;
            assert_localhost_refused(op, &requests, result, foreign_message);
        }

        let unowned = Session::new(SessionType::Shell, None);
        for (op, path) in [
            ("read", foreign_file.as_str()),
            ("write", foreign_file.as_str()),
            ("read", "localhost://Users/self/Documents/secret.txt"),
            ("write", "Users/self/Documents/secret.txt"),
        ] {
            let body = if op == "write" {
                serde_json::json!({"path": path, "content": "shared"})
            } else {
                serde_json::json!({"path": path})
            };
            let (requests, result) =
                dispatch_localhost(unowned.clone(), op, body, HeaderMap::new(), None).await;
            assert_localhost_refused(op, &requests, result, "principal-scoped launch context");
        }

        let foreign_segment = root_b.trim_start_matches("localhost://Users/");
        let climber = format!("localhost://Users/self/../{foreign_segment}/Documents/secret.txt");
        let (requests, result) = dispatch_localhost(
            owned_session(owner_a, SessionType::Shell),
            "read",
            serde_json::json!({"path": climber}),
            HeaderMap::new(),
            Some(manager.clone()),
        )
        .await;
        assert_localhost_refused("read", &requests, result, "escapes the caller space");

        let (requests, result) = dispatch_localhost(
            owned_session(owner_a, SessionType::Shell),
            "read",
            serde_json::json!({"path": "localhost://Users"}),
            HeaderMap::new(),
            Some(manager),
        )
        .await;
        assert_localhost_refused("read", &requests, result, foreign_message);
    }

    #[tokio::test]
    async fn user_a_can_read_and_write_own_localhost_paths() {
        let owner_a = "user-a";
        let root_a = crate::auth::principal_localhost_root(owner_a);
        let own_file = format!("{root_a}/Documents/note.txt");
        let manager = capability_manager();

        for (op, path) in [
            ("read", "localhost://Users/self/Documents/note.txt"),
            ("write", "localhost://Users/self/Documents/note.txt"),
            ("read", own_file.as_str()),
            ("write", own_file.as_str()),
            ("read", "Users/self/Documents/note.txt"),
        ] {
            let body = if op == "write" {
                serde_json::json!({"path": path, "content": "mine"})
            } else {
                serde_json::json!({"path": path})
            };
            let (requests, result) = dispatch_localhost(
                owned_session(owner_a, SessionType::Shell),
                op,
                body,
                HeaderMap::new(),
                Some(manager.clone()),
            )
            .await;
            assert_localhost_scoped(op, &requests, result, &own_file);
            if op == "write" {
                assert_eq!(requests[0]["content"], "mine");
            }
        }

        for op in ["read", "write"] {
            let app = owned_session(owner_a, SessionType::Capsule);
            let action = if op == "write" {
                Action::Write
            } else {
                Action::Read
            };
            let token = manager.grant(
                app.id.as_str(),
                ResourceId::new("localhost://Users/self/Documents/*"),
                action,
                TokenConstraints::default(),
                None,
            );
            let body = if op == "write" {
                serde_json::json!({
                    "path": "localhost://Users/self/Documents/note.txt",
                    "content": "mine"
                })
            } else {
                serde_json::json!({"path": "localhost://Users/self/Documents/note.txt"})
            };
            let (requests, result) =
                dispatch_localhost(app, op, body, token_headers(&token), Some(manager.clone()))
                    .await;
            assert_localhost_scoped(op, &requests, result, &own_file);
        }

        let (requests, result) = dispatch_localhost(
            owned_session(owner_a, SessionType::Shell),
            "read",
            serde_json::json!({"path": "localhost://Local/Shared/note.txt"}),
            HeaderMap::new(),
            Some(manager),
        )
        .await;
        assert_localhost_scoped(
            "read",
            &requests,
            result,
            "localhost://Local/Shared/note.txt",
        );
    }
}
