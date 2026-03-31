use std::sync::Arc;

use elastos_common::localhost::{ensure_file_backed_roots, file_backed_prefixes};
use elastos_runtime::{capability, content, namespace, primitives, provider, session};
use elastos_server::sources::{default_data_dir, local_session_owner};
use elastos_server::{api, fetcher, ownership};

pub(crate) struct ServerInfrastructure {
    pub(crate) audit_log: Arc<primitives::audit::AuditLog>,
    pub(crate) session_registry: Arc<session::SessionRegistry>,
    pub(crate) capability_manager: Arc<capability::CapabilityManager>,
    pub(crate) pending_store: Arc<capability::pending::PendingRequestStore>,
    pub(crate) provider_registry: Arc<provider::ProviderRegistry>,
    pub(crate) namespace_store: Arc<namespace::NamespaceStore>,
    pub(crate) identity_state: Option<api::handlers::identity::IdentityState>,
    pub(crate) tls_config: Option<axum_server::tls_rustls::RustlsConfig>,
    pub(crate) provider_cid: String,
    pub(crate) shell_cid: Option<String>,
    pub(crate) notepad_cid: Option<String>,
}

pub(crate) async fn setup_server_infrastructure() -> anyhow::Result<ServerInfrastructure> {
    setup_server_infrastructure_impl(true).await
}

pub(crate) async fn setup_control_plane_infrastructure() -> anyhow::Result<ServerInfrastructure> {
    setup_server_infrastructure_impl(false).await
}

async fn setup_server_infrastructure_impl(
    spawn_host_providers: bool,
) -> anyhow::Result<ServerInfrastructure> {
    let data_dir = default_data_dir();
    let _ = ownership::repair_path_recursive(&data_dir);

    let audit_log = Arc::new(primitives::audit::AuditLog::new());
    let session_registry = Arc::new(session::SessionRegistry::new(audit_log.clone()));
    session_registry
        .set_default_owner(local_session_owner(&data_dir)?)
        .await;
    let metrics = Arc::new(primitives::metrics::MetricsManager::new());
    let capability_store = Arc::new(capability::CapabilityStore::new());
    let capability_manager = Arc::new(capability::CapabilityManager::load_or_generate(
        &data_dir,
        capability_store,
        audit_log.clone(),
        metrics.clone(),
    ));
    let pending_store = Arc::new(capability::pending::PendingRequestStore::new(
        audit_log.clone(),
    ));

    let tls_config = match elastos_tls::load_or_create_tls_config(&data_dir).await {
        Ok(config) => {
            tracing::info!("TLS enabled (self-signed CA)");
            Some(config)
        }
        Err(e) => {
            tracing::warn!("TLS disabled: {}. Running without HTTPS.", e);
            None
        }
    };

    ensure_file_backed_roots(&data_dir).ok();
    let provider_registry = Arc::new(provider::ProviderRegistry::new());
    let device_key = elastos_identity::load_or_create_device_key(&data_dir)?;
    let mut provider_cid = "sha256:unavailable".to_string();
    if spawn_host_providers {
        let verify_provider_binary = |name: &str, path: &std::path::Path| -> anyhow::Result<()> {
            let checksum = crate::setup::verify_installed_component_binary(&data_dir, name, path)?;
            tracing::info!(
                "{} binary verified against installed manifest ({})",
                name,
                checksum
            );
            Ok(())
        };

        let binary_path =
            crate::find_installed_provider_binary("localhost-provider").ok_or_else(|| {
                anyhow::anyhow!(
                    "localhost-provider not installed.\n  \
                 Run:\n  \
                   elastos setup --with localhost-provider"
                )
            })?;
        verify_provider_binary("localhost-provider", &binary_path)?;

        let provider_bytes = std::fs::read(&binary_path)?;
        provider_cid = format!(
            "sha256:{}",
            hex::encode(elastos_runtime::signature::hash_content(&provider_bytes))
        );

        let device_key_hex = hex::encode(device_key.as_ref());

        let config = provider::BridgeProviderConfig {
            base_path: data_dir.to_string_lossy().to_string(),
            allowed_paths: file_backed_prefixes(),
            read_only: false,
            encryption_key: device_key_hex.clone(),
            ..Default::default()
        };
        let bridge = provider::ProviderBridge::spawn(&binary_path, config)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to spawn localhost-provider capsule: {}.\n  \
                     Reinstall with:\n  \
                       elastos setup --with localhost-provider",
                    e
                )
            })?;
        tracing::info!(
            "localhost-provider capsule {} from {}",
            provider_cid,
            binary_path.display()
        );
        let provider: Arc<dyn provider::Provider> =
            Arc::new(provider::CapsuleProvider::new(Arc::new(bridge)));
        provider_registry.register(provider).await;

        if let Some(path) = crate::find_installed_provider_binary("did-provider") {
            if let Err(e) = verify_provider_binary("did-provider", &path) {
                tracing::warn!("Skipping did-provider due to verification failure: {}", e);
            } else {
                let did_config = provider::BridgeProviderConfig {
                    base_path: data_dir.to_string_lossy().to_string(),
                    allowed_paths: file_backed_prefixes(),
                    read_only: false,
                    encryption_key: device_key_hex.clone(),
                    ..Default::default()
                };
                match provider::ProviderBridge::spawn(&path, did_config).await {
                    Ok(bridge) => {
                        let provider: Arc<dyn provider::Provider> = Arc::new(
                            provider::CapsuleProvider::with_scheme(Arc::new(bridge), "did"),
                        );
                        if let Err(e) = provider_registry
                            .register_sub_provider("did", provider)
                            .await
                        {
                            tracing::warn!("Failed to register elastos://did sub-provider: {}", e);
                        }
                        tracing::info!("did-provider capsule from {}", path.display());
                    }
                    Err(e) => tracing::warn!("Failed to spawn did-provider: {}", e),
                }
            }
        }

        if let Some(path) = crate::find_installed_provider_binary("webspace-provider") {
            if let Err(e) = verify_provider_binary("webspace-provider", &path) {
                tracing::warn!(
                    "Skipping webspace-provider due to verification failure: {}",
                    e
                );
            } else {
                match provider::ProviderBridge::spawn(&path, Default::default()).await {
                    Ok(bridge) => {
                        let provider: Arc<dyn provider::Provider> = Arc::new(
                            provider::CapsuleProvider::with_scheme(Arc::new(bridge), "webspace"),
                        );
                        provider_registry.register(provider).await;
                        tracing::info!("webspace-provider capsule from {}", path.display());
                    }
                    Err(e) => tracing::warn!("Failed to spawn webspace-provider: {}", e),
                }
            }
        }

        let mut llama_endpoint: Option<String> = None;
        if let Some(path) = crate::find_installed_provider_binary("llama-provider") {
            let mut llama_extra = serde_json::Map::new();
            if let Ok(v) = std::env::var("LLAMA_MODEL_PATH") {
                llama_extra.insert("model_path".into(), serde_json::Value::String(v));
            }
            if let Ok(v) = std::env::var("LLAMA_N_CTX") {
                if let Ok(n) = v.parse::<u32>() {
                    llama_extra.insert("n_ctx".into(), serde_json::json!(n));
                }
            }
            if let Ok(v) = std::env::var("LLAMA_GPU_LAYERS") {
                if let Ok(n) = v.parse::<i32>() {
                    llama_extra.insert("n_gpu_layers".into(), serde_json::json!(n));
                }
            }
            if let Ok(v) = std::env::var("LLAMA_MODEL_PROFILE") {
                llama_extra.insert("model_profile".into(), serde_json::Value::String(v));
            }
            let llama_config = provider::BridgeProviderConfig {
                extra: serde_json::Value::Object(llama_extra),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, llama_config).await {
                Ok(bridge) => {
                    let bridge = Arc::new(bridge);
                    let status_req = serde_json::json!({"op": "status"});
                    if let Ok(resp) = bridge.send_raw(&status_req).await {
                        if let Some(ep) = resp
                            .get("data")
                            .and_then(|d| d.get("endpoint"))
                            .and_then(|v| v.as_str())
                        {
                            llama_endpoint = Some(ep.to_string());
                        }
                    }
                    let provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::clone(&bridge), "llama"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("llama", provider)
                        .await
                    {
                        tracing::warn!("Failed to register llama sub-provider: {}", e);
                    }
                    tracing::info!(
                        "llama-provider registered (lazy start — model loads on first request){}",
                        llama_endpoint
                            .as_ref()
                            .map(|ep| format!(", endpoint: {}", ep))
                            .unwrap_or_default()
                    );
                }
                Err(e) => tracing::warn!("llama-provider unavailable: {} (local AI disabled)", e),
            }
        }

        if let Some(path) = crate::find_installed_provider_binary("ai-provider") {
            let mut ai_extra = serde_json::Map::new();
            if let Some(ref ep) = llama_endpoint {
                ai_extra.insert(
                    "local_url".into(),
                    serde_json::Value::String(format!("{}/v1/chat/completions", ep)),
                );
            }
            if let Ok(v) = std::env::var("OLLAMA_URL") {
                if v.starts_with("http://") || v.starts_with("https://") {
                    ai_extra.insert("ollama_url".into(), serde_json::Value::String(v));
                } else {
                    tracing::warn!(
                        "OLLAMA_URL ignored (must start with http:// or https://): {}",
                        v
                    );
                }
            }
            if let Ok(v) = std::env::var("OLLAMA_MODEL") {
                ai_extra.insert("ollama_model".into(), serde_json::Value::String(v));
            }
            if let Ok(v) = std::env::var("VENICE_API_KEY") {
                ai_extra.insert("venice_api_key".into(), serde_json::Value::String(v));
            }
            if let Ok(v) = std::env::var("VENICE_MODEL") {
                ai_extra.insert("venice_model".into(), serde_json::Value::String(v));
            }
            let ai_config = provider::BridgeProviderConfig {
                extra: serde_json::Value::Object(ai_extra),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, ai_config).await {
                Ok(bridge) => {
                    let provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "ai"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("ai", provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://ai sub-provider: {}", e);
                    }
                    tracing::info!("ai-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn ai-provider: {}", e),
            }
        }

        if let Some(path) = crate::find_installed_provider_binary("ipfs-provider") {
            if let Err(e) = verify_provider_binary("ipfs-provider", &path) {
                tracing::warn!("Skipping ipfs-provider due to verification failure: {}", e);
            } else {
                match provider::ProviderBridge::spawn(&path, Default::default()).await {
                    Ok(bridge) => {
                        let bridge = Arc::new(bridge);
                        let ipfs_provider: Arc<dyn provider::Provider> = Arc::new(
                            provider::CapsuleProvider::with_scheme(Arc::clone(&bridge), "ipfs"),
                        );
                        if let Err(e) = provider_registry
                            .register_sub_provider("ipfs", ipfs_provider)
                            .await
                        {
                            tracing::warn!("Failed to register elastos://ipfs sub-provider: {}", e);
                        }
                        tracing::info!("ipfs-provider capsule from {}", path.display());
                    }
                    Err(e) => tracing::debug!("ipfs-provider unavailable: {}", e),
                }
            }
        }
    }

    // Built-in Carrier node — ALWAYS starts, not conditional on spawn_host_providers.
    // Carrier is fundamental infrastructure: gossip, content, identity.
    // Identity is DID (derived from device_key), not raw device_key.
    let (carrier_signing_key, carrier_did) = elastos_identity::derive_did(&device_key);
    {
        match elastos_server::carrier::start_carrier_node(
            &carrier_signing_key,
            &carrier_did,
            data_dir.clone(),
        )
        .await
        {
            Ok(carrier_node) => {
                let gossip_provider: Arc<dyn provider::Provider> =
                    Arc::new(elastos_server::carrier::CarrierGossipProvider::new(
                        carrier_node.gossip_state.clone(),
                    ));
                if let Err(e) = provider_registry
                    .register_sub_provider("peer", gossip_provider)
                    .await
                {
                    tracing::warn!("Failed to register Carrier gossip provider: {}", e);
                }
                // Hold the carrier node alive. Dropping it kills the endpoint.
                tokio::spawn(async move {
                    let _node = carrier_node;
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                    }
                });
                tracing::info!("Carrier node online (P2P + gossip)");
            }
            Err(e) => {
                tracing::warn!("Carrier node failed: {:#}", e);
            }
        }
    }

    let namespace_path = data_dir.join("namespaces");
    std::fs::create_dir_all(&namespace_path).ok();
    let resolver_config = content::ResolverConfig {
        // No ambient public-web fetch in the default trusted server path.
        ipfs_gateways: Vec::new(),
        ..content::ResolverConfig::default()
    };
    let content_resolver = Arc::new(content::ContentResolver::new(
        resolver_config,
        audit_log.clone(),
        Arc::new(fetcher::LoopbackIpfsGatewayFetcher::new()),
    ));
    let namespace_store = Arc::new(namespace::NamespaceStore::new(
        namespace_path,
        content_resolver,
        audit_log.clone(),
    ));

    let identity_state = match elastos_identity::IdentityManager::new(data_dir.clone()) {
        Ok(manager) => {
            tracing::info!("Identity manager initialized (dynamic RP)");
            Some(api::handlers::identity::IdentityState {
                manager: Arc::new(tokio::sync::Mutex::new(manager)),
                session_registry: session_registry.clone(),
                audit_log: Some(audit_log.clone()),
            })
        }
        Err(e) => {
            tracing::warn!("Identity manager disabled: {}", e);
            None
        }
    };

    let shell_cid = crate::find_installed_provider_binary("shell").and_then(|path| {
        std::fs::read(&path).ok().map(|bytes| {
            let cid = format!(
                "sha256:{}",
                hex::encode(elastos_runtime::signature::hash_content(&bytes))
            );
            tracing::info!("shell capsule {} from {}", cid, path.display());
            cid
        })
    });

    let notepad_cid = crate::find_installed_provider_binary("notepad").and_then(|path| {
        std::fs::read(&path).ok().map(|bytes| {
            let cid = format!(
                "sha256:{}",
                hex::encode(elastos_runtime::signature::hash_content(&bytes))
            );
            tracing::debug!("notepad capsule {} from {}", cid, path.display());
            cid
        })
    });

    Ok(ServerInfrastructure {
        audit_log,
        session_registry,
        capability_manager,
        pending_store,
        provider_registry,
        namespace_store,
        identity_state,
        tls_config,
        provider_cid,
        shell_cid,
        notepad_cid,
    })
}
