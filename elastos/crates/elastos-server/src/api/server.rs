//! HTTP server implementation
//!
//! Provides server configurations:
//! - Basic server (no auth, for local-only `elastos serve` without MicroVM)
//! - Server with sessions (full auth + capability flow, used by MicroVM capsules)

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    http::{HeaderName, HeaderValue, Request},
    middleware as axum_middleware,
    routing::{delete, get, head, post, put},
    Extension, Router,
};
use tokio::net::TcpListener;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::services::ServeDir;

use crate::api::handlers::docs::DocsState;
use crate::api::handlers::identity::IdentityState;
use crate::api::handlers::{self, CapabilityState, NamespaceState};
use crate::api::middleware::{
    auth_middleware, rate_limit_middleware, shell_only_middleware, ApiState, RateLimitState,
    RateLimiter,
};
use crate::api::routes;
use crate::runtime::Runtime;
use elastos_runtime::capability::evaluator::ShellPassthroughVerifier;
use elastos_runtime::capability::{
    AutoGrantVerifier, CapabilityManager, PendingRequestStore, PolicyEvaluator, RulesVerifier,
};
use elastos_runtime::namespace::NamespaceStore;
use elastos_runtime::provider::ProviderRegistry;
use elastos_runtime::session::SessionRegistry;

/// Middleware that sets Cross-Origin-Opener-Policy and Cross-Origin-Embedder-Policy headers.
/// Required for SharedArrayBuffer (used by threaded WASM like mgba-wasm).
async fn cross_origin_isolation(
    request: Request<axum::body::Body>,
    next: axum_middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-embedder-policy"),
        HeaderValue::from_static("require-corp"),
    );
    response
}

fn is_allowed_local_origin(origin: &HeaderValue) -> bool {
    let s = match origin.to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    match url::Url::parse(s) {
        Ok(url) => matches!(
            url.host_str(),
            Some("127.0.0.1") | Some("localhost") | Some("::1") | Some("[::1]")
        ),
        Err(_) => false,
    }
}

/// Start the HTTP API server (legacy, no auth)
pub async fn start_server(runtime: Arc<Runtime>, addr: &str) -> anyhow::Result<()> {
    start_server_with_capsules(runtime, addr, None).await
}

/// Start the HTTP API server with optional web capsule serving (legacy, no auth)
pub async fn start_server_with_capsules(
    runtime: Arc<Runtime>,
    addr: &str,
    capsule_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let mut app = Router::new()
        .route("/api/health", get(routes::health))
        .route("/api/capsules", get(routes::list_capsules))
        .route("/api/capsules", post(routes::launch_capsule))
        .route("/api/capsules/:id", delete(routes::stop_capsule))
        .layer(cors.clone())
        .layer(Extension(runtime));

    // Add static file serving for web capsules if directory is provided
    let has_capsule = capsule_dir.is_some();
    if let Some(dir) = capsule_dir {
        tracing::info!("Serving web capsule from: {}", dir.display());
        let serve_dir = ServeDir::new(&dir).append_index_html_on_directories(true);
        app = app
            .layer(axum_middleware::from_fn(cross_origin_isolation))
            .fallback_service(serve_dir);
    }

    let listener = TcpListener::bind(addr).await?;
    if has_capsule {
        tracing::info!("Web capsule server listening on http://{}", addr);
    } else {
        tracing::info!("API server listening on http://{}", addr);
    }

    axum::serve(listener, app).await?;

    Ok(())
}

/// Bootstrap state for web capsules (provides token + manifest info to the frontend)
#[derive(Clone)]
pub struct CapsuleBootstrapState {
    pub token: String,
    pub manifest: elastos_common::CapsuleManifest,
}

/// Configuration for the full HTTP API server (Phase 5+).
pub struct ServerConfig {
    pub runtime: Arc<Runtime>,
    pub session_registry: Arc<SessionRegistry>,
    pub capability_manager: Arc<CapabilityManager>,
    pub pending_store: Arc<PendingRequestStore>,
    pub namespace_store: Option<Arc<NamespaceStore>>,
    pub provider_registry: Option<Arc<ProviderRegistry>>,
    pub audit_log: Option<Arc<elastos_runtime::primitives::audit::AuditLog>>,
    pub identity_state: Option<IdentityState>,
    pub docs_dir: Option<PathBuf>,
    pub addr: String,
    pub capsule_dir: Option<PathBuf>,
    /// Directory containing data capsule files (served at /capsule-data/)
    pub data_dir: Option<PathBuf>,
    /// Bootstrap state for web capsule auto-configuration
    pub bootstrap_state: Option<CapsuleBootstrapState>,
    pub tls_config: Option<axum_server::tls_rustls::RustlsConfig>,
    /// Capsule supervisor for VM-based capsule lifecycle (supervisor path only)
    pub supervisor: Option<Arc<crate::supervisor::Supervisor>>,
    /// Readiness signal — sent after the TCP listener binds successfully.
    /// Replaces startup sleep heuristics with a deterministic handshake.
    pub ready_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// Shared secret for the attach endpoint — callers prove local ownership by
    /// presenting this secret (read from the chmod-600 runtime-coords file) to
    /// mint short-lived session tokens.  When `None` the attach endpoint is disabled.
    pub attach_secret: Option<String>,
}

/// Start the HTTP API server with full session and capability support
///
/// This is the Phase 5+ server configuration that includes:
/// - Session token authentication
/// - Capability request/grant/deny flow
/// - Shell-only endpoints for permission management
/// - Namespace API for content-addressed storage
/// - File-backed localhost API (localhost://<root>/...)
pub async fn start_server_with_sessions(config: ServerConfig) -> anyhow::Result<()> {
    let ServerConfig {
        runtime,
        session_registry,
        capability_manager,
        pending_store,
        namespace_store,
        provider_registry,
        audit_log,
        identity_state,
        docs_dir,
        addr,
        capsule_dir,
        data_dir,
        bootstrap_state,
        tls_config,
        supervisor,
        ready_tx,
        attach_secret,
    } = config;
    // CORS: allow localhost origins for browser-based capsule UIs and
    // local development. Parses the Origin URL and compares the host
    // to prevent bypass via domains like localhost.evil.com.
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            is_allowed_local_origin(origin)
        }))
        .allow_methods(Any)
        .allow_headers(Any);

    // Shared state
    let api_state = ApiState {
        session_registry: session_registry.clone(),
    };

    let shadow_mode = std::env::var("ELASTOS_SHADOW_MODE")
        .unwrap_or_default()
        .to_ascii_lowercase();

    let policy_evaluator = match shadow_mode.as_str() {
        "rules" => {
            tracing::info!("Shadow verification enabled (RulesVerifier)");
            Arc::new(PolicyEvaluator::with_shadow(
                Box::new(ShellPassthroughVerifier),
                Box::new(RulesVerifier::with_defaults()),
                capability_manager.audit_log().clone(),
            ))
        }
        "1" | "true" | "yes" | "on" => {
            tracing::info!("Shadow verification enabled (AutoGrantVerifier)");
            Arc::new(PolicyEvaluator::with_shadow(
                Box::new(ShellPassthroughVerifier),
                Box::new(AutoGrantVerifier),
                capability_manager.audit_log().clone(),
            ))
        }
        _ => Arc::new(PolicyEvaluator::new(
            Box::new(ShellPassthroughVerifier),
            capability_manager.audit_log().clone(),
        )),
    };

    let capability_state = CapabilityState {
        pending_store: pending_store.clone(),
        capability_manager: capability_manager.clone(),
        policy_evaluator,
    };

    // Rate limiters: 100 req/s general, 5 req/s for identity endpoints
    let general_rate_limiter = Arc::new(RateLimiter::new(100.0));
    let identity_rate_limiter = Arc::new(RateLimiter::new(5.0));

    let general_rate_state = RateLimitState {
        session_registry: session_registry.clone(),
        rate_limiter: general_rate_limiter,
    };

    let identity_rate_state = RateLimitState {
        session_registry: session_registry.clone(),
        rate_limiter: identity_rate_limiter,
    };

    // Public routes (no auth required)
    let public_routes = Router::new().route("/api/health", get(routes::health));

    // Attach endpoint — exchanges local secret for a session token.
    let attach_routes = if let Some(secret) = attach_secret {
        let attach_state = handlers::attach::AttachState {
            session_registry: session_registry.clone(),
            secret,
        };
        Router::new()
            .route("/api/auth/attach", post(handlers::attach::attach))
            .with_state(attach_state)
    } else {
        Router::new()
    };

    // Authenticated routes (require valid session token, rate-limited)
    let auth_routes = Router::new()
        .route("/api/session", get(handlers::session_info))
        .route(
            "/api/capability/request",
            post(handlers::request_capability),
        )
        .route("/api/capability/request/:id", get(handlers::request_status))
        .route("/api/capability/list", get(handlers::list_capabilities))
        .layer(axum_middleware::from_fn_with_state(
            general_rate_state.clone(),
            rate_limit_middleware,
        ))
        .layer(axum_middleware::from_fn_with_state(
            api_state.clone(),
            auth_middleware,
        ))
        .with_state(capability_state.clone());

    // Shell-only routes (require shell session)
    let shell_routes = Router::new()
        .route("/api/capability/pending", get(handlers::list_pending))
        .route("/api/capability/grant", post(handlers::grant_request))
        .route("/api/capability/deny", post(handlers::deny_request))
        // Revoke endpoints
        .route("/api/capability/:id", delete(handlers::revoke_capability))
        .route(
            "/api/capability/revoke-all",
            post(handlers::revoke_all_capabilities),
        )
        // Audit log endpoints
        .route("/api/audit", get(handlers::get_audit_log))
        .route("/api/audit/types", get(handlers::get_audit_event_types))
        .layer(axum_middleware::from_fn(shell_only_middleware))
        .layer(axum_middleware::from_fn_with_state(
            api_state.clone(),
            auth_middleware,
        ))
        .with_state(capability_state.clone());

    // Orchestrator routes (shell-only — runtime coordination for attach flow)
    let orchestrator_state = handlers::orchestrator::OrchestratorState {
        session_registry: session_registry.clone(),
    };
    let orchestrator_routes = Router::new()
        .route(
            "/api/orchestrator/session",
            post(handlers::orchestrator::create_session),
        )
        .layer(axum_middleware::from_fn(shell_only_middleware))
        .layer(axum_middleware::from_fn_with_state(
            api_state.clone(),
            auth_middleware,
        ))
        .with_state(orchestrator_state);

    // Supervisor routes (shell-only — capsule lifecycle for VM-based supervisor path)
    let supervisor_routes = if let Some(sup) = supervisor {
        let sup_state = handlers::supervisor_api::SupervisorState { supervisor: sup };
        Router::new()
            .route(
                "/api/supervisor/ensure-external",
                post(handlers::supervisor_api::ensure_external),
            )
            .route(
                "/api/supervisor/ensure-capsule",
                post(handlers::supervisor_api::ensure_capsule),
            )
            .route(
                "/api/supervisor/launch-capsule",
                post(handlers::supervisor_api::launch_capsule),
            )
            .route(
                "/api/supervisor/stop-capsule",
                post(handlers::supervisor_api::stop_capsule),
            )
            .route(
                "/api/supervisor/wait-capsule",
                post(handlers::supervisor_api::wait_capsule),
            )
            .route(
                "/api/supervisor/resolve-plan",
                post(handlers::supervisor_api::resolve_plan),
            )
            .route(
                "/api/supervisor/start-gateway",
                post(handlers::supervisor_api::start_gateway),
            )
            .layer(axum_middleware::from_fn(shell_only_middleware))
            .layer(axum_middleware::from_fn_with_state(
                api_state.clone(),
                auth_middleware,
            ))
            .with_state(sup_state)
    } else {
        Router::new()
    };

    // Namespace routes (require valid session, optional - only if namespace_store is provided)
    let namespace_routes = if let Some(ns_store) = namespace_store {
        let namespace_state = NamespaceState {
            namespace_store: ns_store,
            capability_manager: Some(capability_manager.clone()),
        };

        Router::new()
            .route("/api/namespace/list", get(handlers::list_path))
            .route("/api/namespace/resolve", get(handlers::resolve_path))
            .route("/api/namespace/read", get(handlers::read_content))
            .route("/api/namespace/write", post(handlers::write_content))
            .route("/api/namespace/delete", delete(handlers::delete_path))
            .route("/api/namespace/status", get(handlers::namespace_status))
            .route("/api/namespace/cache", get(handlers::cache_status))
            .route("/api/namespace/prefetch", post(handlers::prefetch_content))
            .layer(axum_middleware::from_fn_with_state(
                api_state.clone(),
                auth_middleware,
            ))
            .with_state(namespace_state)
    } else {
        Router::new()
    };

    // File-backed localhost routes (require valid session, optional - only if provider_registry is provided)
    // Public contract: rooted `localhost://...` paths.
    let storage_routes = if let Some(registry) = provider_registry {
        let storage_state = handlers::storage::ProviderStorageState {
            registry: registry.clone(),
            audit_log: audit_log.clone(),
            capability_manager: Some(capability_manager.clone()),
            storage_quota_mb: 0, // 0 = unlimited (configurable via RuntimeConfig)
        };

        // Generic provider proxy: POST /api/provider/:scheme/:op
        let proxy_state = handlers::provider::ProviderProxyState {
            registry,
            capability_manager: Some(capability_manager.clone()),
        };

        let storage_router = Router::new()
            .route("/api/localhost", get(handlers::storage_get_root))
            .route("/api/localhost/", get(handlers::storage_get_root))
            .route("/api/localhost/*path", get(handlers::storage_get))
            .route("/api/localhost/*path", put(handlers::storage_write))
            .route("/api/localhost/*path", delete(handlers::storage_delete))
            .route("/api/localhost/*path", head(handlers::storage_stat))
            .route("/api/localhost/*path", post(handlers::storage_post))
            .layer(axum_middleware::from_fn_with_state(
                api_state.clone(),
                auth_middleware,
            ))
            .with_state(storage_state);

        // Generic provider proxy route
        let proxy_router = Router::new()
            .route(
                "/api/provider/:scheme/:op",
                post(handlers::provider::provider_proxy),
            )
            .layer(axum_middleware::from_fn_with_state(
                api_state.clone(),
                auth_middleware,
            ))
            .with_state(proxy_state);

        storage_router.merge(proxy_router)
    } else {
        Router::new()
    };

    // Identity routes (require valid session, stricter rate limit: 5 req/s)
    let identity_routes = if let Some(id_state) = identity_state {
        Router::new()
            .route(
                "/api/identity/status",
                get(handlers::identity::identity_status),
            )
            .route(
                "/api/identity/register/begin",
                post(handlers::identity::register_begin),
            )
            .route(
                "/api/identity/register/complete",
                post(handlers::identity::register_complete),
            )
            .route(
                "/api/identity/authenticate/begin",
                post(handlers::identity::authenticate_begin),
            )
            .route(
                "/api/identity/authenticate/complete",
                post(handlers::identity::authenticate_complete),
            )
            .layer(axum_middleware::from_fn_with_state(
                identity_rate_state,
                rate_limit_middleware,
            ))
            .layer(axum_middleware::from_fn_with_state(
                api_state.clone(),
                auth_middleware,
            ))
            .with_state(id_state)
    } else {
        Router::new()
    };

    // Documentation routes (no auth, read-only)
    let docs_routes = if let Some(dir) = docs_dir {
        let docs_state = DocsState {
            docs_dir: Arc::new(dir),
        };
        Router::new()
            .route("/api/docs", get(handlers::docs::list_docs))
            .route("/api/docs/{name}", get(handlers::docs::get_doc))
            .with_state(docs_state)
    } else {
        Router::new()
    };

    // Bootstrap route (no auth — localhost only, returns app token + capsule info)
    let bootstrap_routes = if let Some(bs) = bootstrap_state {
        Router::new().route(
            "/api/capsule/bootstrap",
            get({
                let bs = bs.clone();
                move || async move {
                    axum::Json(serde_json::json!({
                        "token": bs.token,
                        "name": bs.manifest.name,
                        "rom": bs.manifest.entrypoint,
                        "storage": bs.manifest.permissions.storage,
                    }))
                }
            }),
        )
    } else {
        Router::new()
    };

    // Capsule management routes (require shell session — launching/stopping is an orchestrator operation)
    let capsule_mgmt_routes = Router::new()
        .route("/api/capsules", get(routes::list_capsules))
        .route("/api/capsules", post(routes::launch_capsule))
        .route("/api/capsules/:id", delete(routes::stop_capsule))
        .layer(axum_middleware::from_fn(shell_only_middleware))
        .layer(axum_middleware::from_fn_with_state(
            api_state.clone(),
            auth_middleware,
        ))
        .layer(Extension(runtime));

    // Combine all routes
    let mut app = Router::new()
        .merge(public_routes)
        .merge(attach_routes)
        .merge(auth_routes)
        .merge(shell_routes)
        .merge(orchestrator_routes)
        .merge(supervisor_routes)
        .merge(namespace_routes)
        .merge(storage_routes)
        .merge(identity_routes)
        .merge(docs_routes)
        .merge(bootstrap_routes)
        .merge(capsule_mgmt_routes)
        .layer(cors.clone());

    // Add test endpoints in debug builds
    #[cfg(debug_assertions)]
    {
        use crate::api::handlers::test_helpers::{create_test_session, TestState};

        let test_state = TestState {
            session_registry: session_registry.clone(),
        };

        let test_routes = Router::new()
            .route("/api/test/create-session", post(create_test_session))
            .with_state(test_state);

        app = app.merge(test_routes);
        tracing::info!("Test endpoints enabled (debug build)");
    }

    // Add data capsule file serving at /capsule-data/ if data_dir is provided
    if let Some(ref dir) = data_dir {
        tracing::info!("Serving capsule data from: {}", dir.display());
        let data_serve = ServeDir::new(dir);
        app = app.nest_service("/capsule-data", data_serve);
    }

    // Add static file serving for web capsules if directory is provided
    let has_capsule = capsule_dir.is_some();
    if let Some(dir) = capsule_dir {
        tracing::info!("Serving web capsule from: {}", dir.display());
        let serve_dir = ServeDir::new(&dir).append_index_html_on_directories(true);
        app = app.fallback_service(serve_dir);
    }

    // Apply COOP/COEP headers to ALL responses when serving a web capsule.
    // Must be after nest_service/fallback_service so it wraps everything.
    if has_capsule {
        app = app.layer(axum_middleware::from_fn(cross_origin_isolation));
    }

    // Start server with or without TLS
    if let Some(tls_config) = tls_config {
        let socket_addr: std::net::SocketAddr = addr.parse()?;
        tracing::info!("API server listening on https://{} (TLS + sessions)", addr);
        if has_capsule {
            tracing::info!("Web capsule available at https://{}", addr);
        }
        // Signal readiness before blocking on serve (TLS bind is implicit)
        if let Some(tx) = ready_tx {
            let _ = tx.send(());
        }
        axum_server::bind_rustls(socket_addr, tls_config)
            .serve(app.into_make_service())
            .await?;
    } else {
        let listener = TcpListener::bind(&addr).await?;
        tracing::info!("API server listening on http://{} (sessions enabled)", addr);
        if has_capsule {
            tracing::info!("Web capsule available at http://{}", addr);
        }
        // Signal readiness after successful bind, before blocking on serve
        if let Some(tx) = ready_tx {
            let _ = tx.send(());
        }
        axum::serve(listener, app).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_allowed_local_origin;
    use axum::http::HeaderValue;

    #[test]
    fn allows_local_loopback_origins() {
        assert!(is_allowed_local_origin(&HeaderValue::from_static(
            "http://localhost:3000"
        )));
        assert!(is_allowed_local_origin(&HeaderValue::from_static(
            "http://127.0.0.1:3000"
        )));
        assert!(is_allowed_local_origin(&HeaderValue::from_static(
            "http://[::1]:3000"
        )));
    }

    #[test]
    fn rejects_non_local_origins() {
        assert!(!is_allowed_local_origin(&HeaderValue::from_static(
            "http://localhost.evil.com"
        )));
        assert!(!is_allowed_local_origin(&HeaderValue::from_static(
            "https://example.com"
        )));
    }
}
