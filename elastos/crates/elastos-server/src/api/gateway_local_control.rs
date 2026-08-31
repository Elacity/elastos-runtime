use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    middleware as axum_middleware,
    routing::{get, post},
    Json, Router,
};
use elastos_runtime::primitives::audit::AuditLog;
use elastos_runtime::provider::ProviderRegistry;
use elastos_runtime::session::SessionRegistry;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use super::{handlers, middleware, routes};

#[derive(Clone)]
struct GatewayPeerControlState {
    registry: Arc<ProviderRegistry>,
}

pub(crate) struct GatewayLocalControl {
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
    _coords: crate::runtime_control::GatewayRuntimeCoordsGuard,
}

pub(crate) async fn start_gateway_local_control(
    data_dir: &Path,
    registry: Arc<ProviderRegistry>,
) -> anyhow::Result<GatewayLocalControl> {
    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret)
        .map_err(|error| anyhow::anyhow!("OS randomness unavailable: {error}"))?;
    let attach_secret = hex::encode(secret);

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let session_registry = Arc::new(SessionRegistry::new(Arc::new(AuditLog::new())));
    let app = gateway_local_control_router(registry, session_registry, attach_secret.clone());
    let (shutdown, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let coords = crate::runtime_control::RuntimeCoords {
        api_url: format!("http://{address}"),
        attach_secret,
        pid: std::process::id(),
        runtime_kind: crate::runtime_control::RUNTIME_KIND_GATEWAY.to_string(),
        binary_sha256: String::new(),
        policy_sha256: String::new(),
        dependency_sha256: String::new(),
    };
    let coords_guard =
        match crate::runtime_control::publish_gateway_runtime_coords(data_dir, coords) {
            Ok(guard) => guard,
            Err(error) => {
                let _ = shutdown.send(());
                let _ = task.await;
                return Err(error);
            }
        };

    Ok(GatewayLocalControl {
        shutdown: Some(shutdown),
        task: Some(task),
        _coords: coords_guard,
    })
}

impl GatewayLocalControl {
    pub(crate) async fn shutdown(mut self) -> anyhow::Result<()> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.await??;
        }
        Ok(())
    }
}

impl Drop for GatewayLocalControl {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn gateway_local_control_router(
    registry: Arc<ProviderRegistry>,
    session_registry: Arc<SessionRegistry>,
    attach_secret: String,
) -> Router {
    let attach = Router::new()
        .route("/api/auth/attach", post(handlers::attach::attach))
        .with_state(handlers::attach::AttachState {
            session_registry: session_registry.clone(),
            secret: attach_secret,
        });
    let peer = Router::new()
        .route("/api/provider/peer/get_ticket", post(peer_get_ticket))
        .layer(axum_middleware::from_fn(middleware::shell_only_middleware))
        .layer(axum_middleware::from_fn_with_state(
            middleware::ApiState { session_registry },
            middleware::auth_middleware,
        ))
        .with_state(GatewayPeerControlState { registry });
    Router::new()
        .route("/api/health", get(routes::health))
        .merge(attach)
        .merge(peer)
}

async fn peer_get_ticket(
    State(state): State<GatewayPeerControlState>,
    body: String,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let value: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid request body".to_string()))?;
    if value.as_object().is_none_or(|object| !object.is_empty()) {
        return Err((
            StatusCode::BAD_REQUEST,
            "get_ticket accepts only an empty object".to_string(),
        ));
    }
    state
        .registry
        .send_raw("peer", &serde_json::json!({"op": "get_ticket"}))
        .await
        .map(Json)
        .map_err(|_| {
            (
                StatusCode::BAD_GATEWAY,
                "local Carrier Provider get_ticket failed".to_string(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::provider::{Provider, ProviderError, ResourceRequest, ResourceResponse};
    use std::sync::Mutex;

    struct TicketProvider {
        requests: Mutex<Vec<serde_json::Value>>,
    }

    #[async_trait::async_trait]
    impl Provider for TicketProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider("raw requests only".to_string()))
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec!["elastos"]
        }

        fn name(&self) -> &'static str {
            "gateway-ticket-test"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(serde_json::json!({
                "status": "ok",
                "data": {
                    "ticket": "private-test-ticket",
                    "node_id": "private-test-node"
                }
            }))
        }
    }

    #[tokio::test]
    async fn gateway_coordinate_is_exact_private_narrow_and_cleaned_up() {
        let temp = tempfile::tempdir().unwrap();
        let provider = Arc::new(TicketProvider {
            requests: Mutex::new(Vec::new()),
        });
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_sub_provider("peer", provider.clone())
            .await
            .unwrap();

        let control = start_gateway_local_control(temp.path(), registry)
            .await
            .unwrap();
        let coordinate_path = crate::runtime_control::gateway_runtime_coord_path(temp.path());
        let coords = crate::runtime_control::read_attachable_runtime_coords(
            temp.path(),
            crate::runtime_control::AttachableRuntimeKind::Gateway,
        )
        .await
        .unwrap();
        assert!(coords.is_gateway_runtime());
        assert_eq!(coords.pid, std::process::id());
        assert!(coords.api_url.starts_with("http://127.0.0.1:"));
        assert!(!coords.attach_secret.is_empty());
        assert!(crate::runtime_control::read_attachable_runtime_coords(
            temp.path(),
            crate::runtime_control::AttachableRuntimeKind::Operator,
        )
        .await
        .is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&coordinate_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        let (ticket, node_id) = crate::operator_control::fetch_local_carrier_bootstrap(
            temp.path(),
            crate::runtime_control::AttachableRuntimeKind::Gateway,
        )
        .await
        .unwrap();
        assert_eq!(ticket, "private-test-ticket");
        assert_eq!(node_id, "private-test-node");
        assert_eq!(
            provider.requests.lock().unwrap().as_slice(),
            &[serde_json::json!({"op": "get_ticket"})]
        );

        let mut operator_coords = coords.clone();
        operator_coords.runtime_kind = crate::runtime_control::RUNTIME_KIND_OPERATOR.to_string();
        crate::runtime_control::write_runtime_coords(
            &crate::runtime_control::runtime_coord_path(temp.path()),
            &operator_coords,
        )
        .unwrap();
        assert_eq!(
            crate::operator_control::fetch_local_carrier_bootstrap(
                temp.path(),
                crate::runtime_control::AttachableRuntimeKind::Operator,
            )
            .await
            .unwrap(),
            (
                "private-test-ticket".to_string(),
                "private-test-node".to_string()
            )
        );

        let client = reqwest::Client::new();
        let attach = client
            .post(format!("{}/api/auth/attach", coords.api_url))
            .json(&serde_json::json!({
                "secret": coords.attach_secret,
                "scope": "shell"
            }))
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();
        let token = attach["token"].as_str().unwrap();
        let response = client
            .post(format!("{}/api/provider/peer/list_peers", coords.api_url))
            .bearer_auth(token)
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(provider.requests.lock().unwrap().len(), 2);

        for forbidden in ["identity", "device.key", "wallet", "chat", "room"] {
            assert!(!temp.path().join(forbidden).exists());
        }
        control.shutdown().await.unwrap();
        assert!(!coordinate_path.exists());
    }

    #[tokio::test]
    async fn shutdown_does_not_remove_a_foreign_start_coordinate() {
        let temp = tempfile::tempdir().unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let control = start_gateway_local_control(temp.path(), registry)
            .await
            .unwrap();
        let coordinate_path = crate::runtime_control::gateway_runtime_coord_path(temp.path());
        let mut replacement: crate::runtime_control::RuntimeCoords =
            serde_json::from_slice(&std::fs::read(&coordinate_path).unwrap()).unwrap();
        replacement.attach_secret = "foreign-start-secret".to_string();
        crate::runtime_control::write_runtime_coords(&coordinate_path, &replacement).unwrap();
        control.shutdown().await.unwrap();
        assert_eq!(
            serde_json::from_slice::<crate::runtime_control::RuntimeCoords>(
                &std::fs::read(&coordinate_path).unwrap()
            )
            .unwrap(),
            replacement
        );
    }

    #[tokio::test]
    async fn stale_and_substituted_gateway_coordinates_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let control = start_gateway_local_control(temp.path(), Arc::new(ProviderRegistry::new()))
            .await
            .unwrap();
        let coordinate_path = crate::runtime_control::gateway_runtime_coord_path(temp.path());
        let original: crate::runtime_control::RuntimeCoords =
            serde_json::from_slice(&std::fs::read(&coordinate_path).unwrap()).unwrap();

        let mut substituted = original.clone();
        substituted.runtime_kind = crate::runtime_control::RUNTIME_KIND_MANAGED_HOME.to_string();
        crate::runtime_control::write_runtime_coords(&coordinate_path, &substituted).unwrap();
        assert!(crate::runtime_control::read_attachable_runtime_coords(
            temp.path(),
            crate::runtime_control::AttachableRuntimeKind::Gateway,
        )
        .await
        .is_none());

        let mut stale = original;
        stale.pid = u32::MAX;
        crate::runtime_control::write_runtime_coords(&coordinate_path, &stale).unwrap();
        assert!(crate::runtime_control::read_attachable_runtime_coords(
            temp.path(),
            crate::runtime_control::AttachableRuntimeKind::Gateway,
        )
        .await
        .is_none());
        assert!(!coordinate_path.exists());
        control.shutdown().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn gateway_coordinate_writer_rejects_a_symlink_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("foreign.json");
        std::fs::write(&target, b"foreign-bytes").unwrap();
        symlink(
            &target,
            crate::runtime_control::gateway_runtime_coord_path(temp.path()),
        )
        .unwrap();

        assert!(
            start_gateway_local_control(temp.path(), Arc::new(ProviderRegistry::new()))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(target).unwrap(), b"foreign-bytes");
    }
}
