use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use elastos_common::localhost::{ensure_file_backed_roots, file_backed_prefixes};
use elastos_runtime::provider::{
    ProviderInvocation, ProviderInvocationTransport, ProviderTransfer,
};
use elastos_runtime::{capability, content, namespace, primitives, provider, session};
use elastos_server::content::ContentProvider;
use elastos_server::documents::DocumentsProvider;
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
    pub(crate) host_helpers: Vec<api::server::HostHelperProcess>,
    /// Per-act spend metering for the act-over-MCP path; `None` ⇒ unmetered (default).
    pub(crate) spend_policy: Option<elastos_server::carrier_bridge::SpendPolicy>,
}

/// Opt-in durable, verified-on-open audit log (the EU AI Act custody mode). Unset → in-memory.
const AUDIT_LOG_PATH_ENV: &str = "ELASTOS_AUDIT_LOG_PATH";
/// Opt-in per-capsule act budget for the act-over-MCP path. Unset/empty → unmetered; an explicit
/// integer (incl. `0`, which hard-stops all acts) enables fail-closed metering.
const DEFAULT_SPEND_BUDGET_ENV: &str = "ELASTOS_DEFAULT_SPEND_BUDGET";
const CONTENT_REPAIR_SCHEDULER_ENV: &str = "ELASTOS_CONTENT_REPAIR_SCHEDULER";
const CONTENT_REPAIR_SCHEDULER_INTERVAL_ENV: &str = "ELASTOS_CONTENT_REPAIR_INTERVAL_SECS";
const CONTENT_REPAIR_SCHEDULER_LIMIT_ENV: &str = "ELASTOS_CONTENT_REPAIR_LIMIT";
const CONTENT_REPAIR_SCHEDULER_MAX_ATTEMPTS_ENV: &str = "ELASTOS_CONTENT_REPAIR_MAX_ATTEMPTS";
const CONTENT_REPAIR_SCHEDULER_FAILURE_BUDGET_ENV: &str = "ELASTOS_CONTENT_REPAIR_FAILURE_BUDGET";
const CONTENT_REPAIR_SCHEDULER_INCLUDE_HEALTHY_ENV: &str = "ELASTOS_CONTENT_REPAIR_INCLUDE_HEALTHY";
const CONTENT_REPAIR_SCHEDULER_MIN_INTERVAL_SECS: u64 = 60;
const CONTENT_REPAIR_SCHEDULER_DEFAULT_INTERVAL_SECS: u64 = 15 * 60;
const CONTENT_REPAIR_SCHEDULER_DEFAULT_LIMIT: u64 = 10;
const CONTENT_REPAIR_SCHEDULER_DEFAULT_MAX_ATTEMPTS: u64 = 3;
const CONTENT_REPAIR_SCHEDULER_DEFAULT_FAILURE_BUDGET: u64 = 5;

pub(crate) async fn setup_server_infrastructure() -> anyhow::Result<ServerInfrastructure> {
    setup_server_infrastructure_impl(true).await
}

pub(crate) async fn setup_control_plane_infrastructure() -> anyhow::Result<ServerInfrastructure> {
    setup_server_infrastructure_impl(false).await
}

/// Build the server's audit log.
///
/// DEFAULT (unset env): the in-memory fail-loud chain — no per-emit `fsync` on the hot path. Making
/// the default file-backed would force an fsync on every capability validate; that perf change waits
/// on the audit group-commit rewrite (KNOWN_GAPS G8, measure-first).
///
/// DURABLE CUSTODY MODE (`ELASTOS_AUDIT_LOG_PATH` set): the log is file-backed AND **verified on
/// open** — if the existing on-disk chain fails the hash + signature walk, startup ABORTS rather
/// than appending a fresh, valid-looking tail onto a tampered history. This is the EU AI Act
/// durable audit trail an operator opts into. The path may be absolute or relative to `data_dir`.
fn build_audit_log(data_dir: &Path) -> anyhow::Result<Arc<primitives::audit::AuditLog>> {
    match std::env::var_os(AUDIT_LOG_PATH_ENV) {
        Some(raw) if !raw.is_empty() => {
            let configured = PathBuf::from(raw);
            let path = if configured.is_absolute() {
                configured
            } else {
                data_dir.join(configured)
            };
            let log = primitives::audit::AuditLog::with_file_verified(&path).map_err(|e| {
                anyhow::anyhow!(
                    "durable audit log at {} failed to open or verify ({e}); set {} only to a \
                     trusted, untampered custody log",
                    path.display(),
                    AUDIT_LOG_PATH_ENV
                )
            })?;
            tracing::info!(
                "Durable audit log enabled (verified-on-open): {}",
                path.display()
            );
            Ok(Arc::new(log))
        }
        _ => Ok(Arc::new(primitives::audit::AuditLog::new())),
    }
}

/// Build the act-over-MCP spend policy from `ELASTOS_DEFAULT_SPEND_BUDGET`.
///
/// Unset or empty ⇒ `None` (unmetered — today's behavior). An explicit non-negative integer enables
/// metering with that per-capsule default budget; `0` is a valid value that fail-closes every act. A
/// non-integer value is a hard configuration error (fail-closed: refuse to start rather than run
/// unmetered against an operator who believed they had enabled a budget).
fn build_spend_policy() -> anyhow::Result<Option<elastos_server::carrier_bridge::SpendPolicy>> {
    match std::env::var(DEFAULT_SPEND_BUDGET_ENV) {
        Ok(raw) if !raw.trim().is_empty() => {
            let default_budget: u64 = raw.trim().parse().map_err(|e| {
                anyhow::anyhow!(
                    "{DEFAULT_SPEND_BUDGET_ENV}={raw:?} is not a non-negative integer ({e})"
                )
            })?;
            tracing::info!(
                "Act spend metering enabled: default per-capsule budget {default_budget}"
            );
            Ok(Some(elastos_server::carrier_bridge::SpendPolicy {
                meter: Arc::new(primitives::spend::SpendMeter::new()),
                default_budget,
            }))
        }
        _ => Ok(None),
    }
}

async fn setup_server_infrastructure_impl(
    spawn_host_providers: bool,
) -> anyhow::Result<ServerInfrastructure> {
    let data_dir = default_data_dir();
    let _ = ownership::repair_path_recursive(&data_dir);

    let audit_log = build_audit_log(&data_dir)?;
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
    let mut managed_host_processes = Vec::new();
    let mut external_availability_registered = false;
    let content_provider = Arc::new(ContentProvider::new(
        data_dir.clone(),
        Arc::downgrade(&provider_registry),
    ));
    provider_registry.register(content_provider.clone()).await;
    if let Err(err) = provider_registry
        .register_sub_provider("content", content_provider)
        .await
    {
        tracing::warn!("Failed to register elastos://content sub-provider: {}", err);
    }
    provider_registry
        .register(Arc::new(DocumentsProvider::new(
            data_dir.clone(),
            Arc::downgrade(&provider_registry),
        )))
        .await;
    let inspect_source: Arc<dyn elastos_server::inspect_provider::InspectSource> = Arc::new(
        elastos_server::inspect_provider::AggregateInspectSource::new(vec![
            Arc::new(elastos_server::inspect_provider::CatalogInspectSource::new(
                data_dir.join("capsules"),
                Arc::downgrade(&provider_registry),
            )),
            Arc::new(
                elastos_server::inspect_provider::RegistryInspectSource::new(Arc::downgrade(
                    &provider_registry,
                )),
            ),
        ]),
    );
    let inspect_provider: Arc<dyn provider::Provider> = Arc::new(
        elastos_server::inspect_provider::InspectProvider::with_registry(
            inspect_source,
            Arc::downgrade(&provider_registry),
        ),
    );
    provider_registry.register(inspect_provider.clone()).await;
    if let Err(err) = provider_registry
        .register_sub_provider("inspect", inspect_provider)
        .await
    {
        tracing::warn!("Failed to register elastos://inspect sub-provider: {}", err);
    }
    let device_key = elastos_identity::load_or_create_device_key(&data_dir)?;
    let device_key_hex = hex::encode(device_key.as_ref());
    let mut provider_cid = "sha256:unavailable".to_string();
    let verify_provider_binary = |name: &str, path: &std::path::Path| -> anyhow::Result<()> {
        // OPERATOR/DEV OVERRIDE (Principle #6 — explicit operator boundary): when the
        // operator EXPLICITLY points at a provider binary via `ELASTOS_<NAME>_BIN`, that
        // env is itself an explicit trust decision, so we honor it WITHOUT the signed
        // installed-manifest check. That check only covers verified install platforms
        // (currently linux); a macOS dev host has no manifest entry. This stays principled
        // because it is narrow (one named binary the operator chose), loud (warns), and
        // NON-AMBIENT (#3) — with no env set, the default install path still fails closed.
        let env_name = format!(
            "ELASTOS_{}_BIN",
            name.to_ascii_uppercase().replace('-', "_")
        );
        if let Some(override_path) = std::env::var_os(&env_name) {
            let same = std::fs::canonicalize(&override_path)
                .ok()
                .zip(std::fs::canonicalize(path).ok())
                .map(|(a, b)| a == b)
                .unwrap_or(false);
            if same {
                tracing::warn!(
                    "{name}: trusting operator-provided binary from {env_name} WITHOUT manifest \
                     verification (explicit dev override) — {}",
                    path.display()
                );
                return Ok(());
            }
        }
        let checksum = crate::setup::verify_installed_component_binary(&data_dir, name, path)?;
        tracing::info!(
            "{} binary verified against installed manifest ({})",
            name,
            checksum
        );
        Ok(())
    };

    if spawn_host_providers {
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

        if let Some(availability_config) = availability_provider_config_from_env() {
            if let Some(path) = crate::find_installed_provider_binary("availability-provider") {
                if let Err(e) = verify_provider_binary("availability-provider", &path) {
                    tracing::warn!(
                        "Skipping availability-provider due to verification failure: {}",
                        e
                    );
                } else {
                    let config = provider::BridgeProviderConfig {
                        extra: availability_config,
                        ..Default::default()
                    };
                    match provider::ProviderBridge::spawn(&path, config).await {
                        Ok(bridge) => {
                            let availability_provider: Arc<dyn provider::Provider> =
                                Arc::new(provider::CapsuleProvider::with_scheme(
                                    Arc::new(bridge),
                                    "availability",
                                ));
                            if let Err(e) = provider_registry
                                .register_sub_provider("availability", availability_provider)
                                .await
                            {
                                tracing::warn!(
                                    "Failed to register elastos://availability sub-provider: {}",
                                    e
                                );
                            } else {
                                external_availability_registered = true;
                            }
                            tracing::info!("availability-provider capsule from {}", path.display());
                        }
                        Err(e) => tracing::warn!("Failed to spawn availability-provider: {}", e),
                    }
                }
            } else {
                tracing::warn!(
                    "Availability provider configured but availability-provider binary is not installed"
                );
            }
        }
    }

    if let Some(path) = crate::find_installed_provider_binary("content-block-graph-provider") {
        if let Err(e) = verify_provider_binary("content-block-graph-provider", &path) {
            tracing::warn!(
                "Skipping content-block-graph-provider due to verification failure: {}",
                e
            );
        } else {
            let block_graph_config = provider::BridgeProviderConfig {
                base_path: data_dir.to_string_lossy().to_string(),
                extra: serde_json::json!({
                    "backend": "kubo_coord"
                }),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, block_graph_config).await {
                Ok(bridge) => {
                    let block_graph_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "block-graph"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("block-graph", block_graph_provider)
                        .await
                    {
                        tracing::warn!(
                            "Failed to register elastos://block-graph sub-provider: {}",
                            e
                        );
                    }
                    tracing::info!(
                        "content-block-graph-provider capsule from {}",
                        path.display()
                    );
                }
                Err(e) => tracing::warn!("Failed to spawn content-block-graph-provider: {}", e),
            }
        }
    } else {
        tracing::warn!(
            "content-block-graph-provider binary is not installed; arbitrary DAG repair will fail closed"
        );
    }

    if let Some(path) = crate::find_installed_provider_binary("object-provider") {
        if let Err(e) = verify_provider_binary("object-provider", &path) {
            tracing::warn!(
                "Skipping {} due to verification failure: {}",
                "object-provider",
                e
            );
        } else {
            let object_config = provider::BridgeProviderConfig {
                base_path: data_dir.to_string_lossy().to_string(),
                allowed_paths: file_backed_prefixes(),
                read_only: false,
                encryption_key: device_key_hex.clone(),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, object_config).await {
                Ok(bridge) => {
                    let bridge = Arc::new(bridge);
                    let object_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(bridge.clone(), "object"),
                    );
                    provider_registry.register(object_provider).await;
                    tracing::info!(
                        "object-provider capsule from {} registered as object provider",
                        path.display()
                    );
                }
                Err(e) => tracing::warn!("Failed to spawn object-provider: {}", e),
            }
        }
    } else {
        tracing::warn!(
            "object-provider binary is not installed; Library object operations will fail closed"
        );
    }

    if let Some(path) = crate::find_installed_provider_binary("webspace-provider") {
        if let Err(e) = verify_provider_binary("webspace-provider", &path) {
            tracing::warn!(
                "Skipping webspace-provider due to verification failure: {}",
                e
            );
        } else {
            let webspace_config = provider::BridgeProviderConfig {
                base_path: data_dir.to_string_lossy().to_string(),
                read_only: false,
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, webspace_config).await {
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
    } else {
        tracing::warn!(
            "webspace-provider binary is not installed; WebSpace roots will fail closed"
        );
    }

    if let Some(path) = crate::find_installed_provider_binary("operator-drive-adapter") {
        if let Err(e) = verify_provider_binary("operator-drive-adapter", &path) {
            tracing::warn!(
                "Skipping operator-drive-adapter due to verification failure: {}",
                e
            );
        } else {
            let adapter_config = provider::BridgeProviderConfig {
                base_path: data_dir.to_string_lossy().to_string(),
                read_only: false,
                extra: operator_drive_adapter_config_from_env(&data_dir)
                    .unwrap_or(serde_json::Value::Null),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, adapter_config).await {
                Ok(bridge) => {
                    let provider: Arc<dyn provider::Provider> =
                        Arc::new(provider::CapsuleProvider::with_scheme(
                            Arc::new(bridge),
                            "operator-drive-adapter",
                        ));
                    provider_registry.register(provider).await;
                    tracing::info!("operator-drive-adapter capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn operator-drive-adapter: {}", e),
            }
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
                Err(e) => tracing::warn!("ipfs-provider unavailable: {}", e),
            }
        }
    } else {
        tracing::warn!(
            "ipfs-provider binary is not installed; elastos://content publish/fetch will fail closed"
        );
    }

    if let Some(path) = crate::find_installed_provider_binary("chain-provider") {
        if let Err(e) = verify_provider_binary("chain-provider", &path) {
            tracing::warn!("Skipping chain-provider due to verification failure: {}", e);
        } else {
            match provider::ProviderBridge::spawn(&path, Default::default()).await {
                Ok(bridge) => {
                    let chain_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "chain"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("chain", chain_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://chain sub-provider: {}", e);
                    }
                    tracing::info!("chain-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn chain-provider: {}", e),
            }
        }
    }

    if let Some(path) = crate::find_installed_provider_binary("encrypt-provider") {
        if let Err(e) = verify_provider_binary("encrypt-provider", &path) {
            tracing::warn!(
                "Skipping encrypt-provider due to verification failure: {}",
                e
            );
        } else {
            match provider::ProviderBridge::spawn(&path, Default::default()).await {
                Ok(bridge) => {
                    let encrypt_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "encrypt"),
                    );
                    // AUD-6: `encrypt` (CEK escrow) is boot-critical. A spawned-but-unregisterable
                    // provider is an INVARIANT violation (a dark mint path), not an optional-absent
                    // binary — fail the boot LOUD rather than warn-and-continue. (Absent binary is
                    // still the outer `else` warn: genuinely optional in a build without it.)
                    provider_registry
                        .register_sub_provider("encrypt", encrypt_provider)
                        .await
                        .map_err(|e| {
                            anyhow::anyhow!(
                                "boot-critical sub-provider 'encrypt' (CEK escrow) spawned but \
                                 failed to register: {e} (AUD-6: refusing to boot with a dark \
                                 mint path)"
                            )
                        })?;
                    tracing::info!("encrypt-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn encrypt-provider: {}", e),
            }
        }
    } else {
        tracing::warn!(
            "encrypt-provider binary is not installed; the Create portal mint path will fail closed"
        );
    }

    if let Some(path) = crate::find_installed_provider_binary("publish-provider") {
        if let Err(e) = verify_provider_binary("publish-provider", &path) {
            tracing::warn!(
                "Skipping publish-provider due to verification failure: {}",
                e
            );
        } else {
            match provider::ProviderBridge::spawn(&path, Default::default()).await {
                Ok(bridge) => {
                    let publish_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "publish"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("publish", publish_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://publish sub-provider: {}", e);
                    }
                    tracing::info!("publish-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn publish-provider: {}", e),
            }
        }
    } else {
        tracing::warn!(
            "publish-provider binary is not installed; the Create portal mint path will fail closed"
        );
    }

    // media-provider (Create portal, video/audio): the runtime-native analogue of PC2's
    // ffmpeg transcode + DASH fragmentation. Holds NO key material — it returns PLAINTEXT
    // segments + track metadata; CENC + dKMS escrow happen in encrypt-provider. ffmpeg path
    // + scratch dir come from the operator config `ELASTOS_MEDIA_PROVIDER_CONFIG` (inherited
    // by the spawned child); unconfigured ⇒ the provider's `package` op fails closed.
    if let Some(path) = crate::find_installed_provider_binary("media-provider") {
        if let Err(e) = verify_provider_binary("media-provider", &path) {
            tracing::warn!("Skipping media-provider due to verification failure: {}", e);
        } else {
            match provider::ProviderBridge::spawn(&path, Default::default()).await {
                Ok(bridge) => {
                    let media_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "media"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("media", media_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://media sub-provider: {}", e);
                    }
                    tracing::info!("media-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn media-provider: {}", e),
            }
        }
    } else {
        tracing::warn!(
            "media-provider binary is not installed; the Create portal media (video/audio) path will fail closed"
        );
    }

    if let Some(path) = crate::find_installed_provider_binary("net-provider") {
        if let Err(e) = verify_provider_binary("net-provider", &path) {
            tracing::warn!("Skipping net-provider due to verification failure: {}", e);
        } else {
            match provider::ProviderBridge::spawn(&path, Default::default()).await {
                Ok(bridge) => {
                    let net_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "net"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("net", net_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://net sub-provider: {}", e);
                    }
                    tracing::info!("net-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn net-provider: {}", e),
            }
        }
    }

    if let Some(local_exit_config) = browser_local_exit_config_from_env(&data_dir) {
        if let Some(path) = crate::find_installed_provider_binary("browser-local-exit") {
            if let Err(e) = verify_provider_binary("browser-local-exit", &path) {
                tracing::warn!(
                    "Skipping browser-local-exit due to verification failure: {}",
                    e
                );
            } else {
                match spawn_browser_local_exit(&path, &local_exit_config) {
                    Ok(child) => {
                        tracing::info!("browser-local-exit helper from {}", path.display());
                        managed_host_processes.push(api::server::HostHelperProcess {
                            name: "browser-local-exit",
                            child,
                        });
                    }
                    Err(e) => tracing::warn!("Failed to spawn browser-local-exit: {}", e),
                }
            }
        } else {
            tracing::warn!(
                "Browser local Exit configured but browser-local-exit binary is not installed"
            );
        }
    }

    if let Some(path) = crate::find_installed_provider_binary("exit-provider") {
        if let Err(e) = verify_provider_binary("exit-provider", &path) {
            tracing::warn!("Skipping exit-provider due to verification failure: {}", e);
        } else {
            let exit_config = provider::BridgeProviderConfig {
                extra: exit_provider_config_from_env(&data_dir).unwrap_or(serde_json::Value::Null),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, exit_config).await {
                Ok(bridge) => {
                    let exit_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "exit"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("exit", exit_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://exit sub-provider: {}", e);
                    }
                    tracing::info!("exit-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn exit-provider: {}", e),
            }
        }
    }

    if let Some(path) = crate::find_installed_provider_binary("browser-engine-adapter") {
        if let Err(e) = verify_provider_binary("browser-engine-adapter", &path) {
            tracing::warn!(
                "Skipping browser-engine-adapter due to verification failure: {}",
                e
            );
        } else {
            let browser_engine_config = provider::BridgeProviderConfig {
                extra: browser_engine_adapter_config_from_env(&data_dir)
                    .unwrap_or(serde_json::Value::Null),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, browser_engine_config).await {
                Ok(bridge) => {
                    let browser_engine_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "browser-engine"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("browser-engine", browser_engine_provider)
                        .await
                    {
                        tracing::warn!(
                            "Failed to register elastos://browser-engine sub-provider: {}",
                            e
                        );
                    }
                    tracing::info!("browser-engine-adapter capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn browser-engine-adapter: {}", e),
            }
        }
    }

    if let Some(path) = crate::find_installed_provider_binary("wallet-provider") {
        if let Err(e) = verify_provider_binary("wallet-provider", &path) {
            tracing::warn!(
                "Skipping wallet-provider due to verification failure: {}",
                e
            );
        } else {
            let wallet_config = provider::BridgeProviderConfig {
                base_path: data_dir.to_string_lossy().to_string(),
                allowed_paths: file_backed_prefixes(),
                read_only: false,
                encryption_key: hex::encode(&device_key),
                ..Default::default()
            };
            match provider::ProviderBridge::spawn(&path, wallet_config).await {
                Ok(bridge) => {
                    let wallet_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "wallet"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("wallet", wallet_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://wallet sub-provider: {}", e);
                    }
                    tracing::info!("wallet-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn wallet-provider: {}", e),
            }
        }
    }

    if let Some(path) = crate::find_installed_provider_binary("drm-provider") {
        if let Err(e) = verify_provider_binary("drm-provider", &path) {
            tracing::warn!("Skipping drm-provider due to verification failure: {}", e);
        } else {
            match provider::ProviderBridge::spawn(&path, Default::default()).await {
                Ok(bridge) => {
                    let drm_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "drm"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("drm", drm_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://drm sub-provider: {}", e);
                    }
                    tracing::info!("drm-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn drm-provider: {}", e),
            }
        }
    }

    if let Some(path) = crate::find_installed_provider_binary("rights-provider") {
        if let Err(e) = verify_provider_binary("rights-provider", &path) {
            tracing::warn!(
                "Skipping rights-provider due to verification failure: {}",
                e
            );
        } else {
            match provider::ProviderBridge::spawn(&path, Default::default()).await {
                Ok(bridge) => {
                    let rights_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "rights"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("rights", rights_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://rights sub-provider: {}", e);
                    }
                    tracing::info!("rights-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn rights-provider: {}", e),
            }
        }
    }

    if let Some(path) = crate::find_installed_provider_binary("key-provider") {
        if let Err(e) = verify_provider_binary("key-provider", &path) {
            tracing::warn!("Skipping key-provider due to verification failure: {}", e);
        } else {
            match provider::ProviderBridge::spawn(&path, Default::default()).await {
                Ok(bridge) => {
                    let key_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "key"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("key", key_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://key sub-provider: {}", e);
                    }
                    tracing::info!("key-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn key-provider: {}", e),
            }
        }
    }

    if let Some(path) = crate::find_installed_provider_binary("decrypt-provider") {
        if let Err(e) = verify_provider_binary("decrypt-provider", &path) {
            tracing::warn!(
                "Skipping decrypt-provider due to verification failure: {}",
                e
            );
        } else {
            match provider::ProviderBridge::spawn(&path, Default::default()).await {
                Ok(bridge) => {
                    let decrypt_provider: Arc<dyn provider::Provider> = Arc::new(
                        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "decrypt"),
                    );
                    if let Err(e) = provider_registry
                        .register_sub_provider("decrypt", decrypt_provider)
                        .await
                    {
                        tracing::warn!("Failed to register elastos://decrypt sub-provider: {}", e);
                    }
                    tracing::info!("decrypt-provider capsule from {}", path.display());
                }
                Err(e) => tracing::warn!("Failed to spawn decrypt-provider: {}", e),
            }
        }
    }

    // Built-in Carrier node — ALWAYS starts, not conditional on spawn_host_providers.
    // Carrier is fundamental infrastructure: gossip, content, identity.
    // Identity is DID (derived from device_key), not raw device_key.
    let (carrier_signing_key, carrier_did) = elastos_identity::derive_did(&device_key);
    {
        match elastos_server::carrier::start_carrier_node_with_registry(
            &carrier_signing_key,
            &carrier_did,
            data_dir.clone(),
            Some(Arc::downgrade(&provider_registry)),
        )
        .await
        {
            Ok(carrier_node) => {
                provider_registry
                    .set_carrier_invoker(Arc::new(
                        elastos_server::carrier::CarrierProviderInvoker::new(),
                    ))
                    .await;
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
                if !external_availability_registered {
                    let availability_provider: Arc<dyn provider::Provider> =
                        Arc::new(
                            elastos_server::carrier::CarrierAvailabilityProvider::with_provider_registry_data_dir_and_peer_attestation_exchange_config(
                            carrier_node.gossip_state.clone(),
                            Arc::downgrade(&provider_registry),
                            data_dir.clone(),
                            carrier_peer_attestation_exchange_config_from_env(),
                        ));
                    if let Err(e) = provider_registry
                        .register_sub_provider("availability", availability_provider)
                        .await
                    {
                        tracing::warn!("Failed to register Carrier availability provider: {}", e);
                    }
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

    maybe_spawn_content_repair_scheduler(provider_registry.clone());

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
                data_dir: data_dir.clone(),
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
        host_helpers: managed_host_processes,
        spend_policy: build_spend_policy()?,
    })
}

fn spawn_browser_local_exit(path: &Path, config: &serde_json::Value) -> anyhow::Result<Child> {
    let relay_path = browser_local_exit_relay_path(config)?;
    if config
        .get("replace_existing_socket")
        .and_then(|value| value.as_bool())
        == Some(true)
    {
        remove_existing_browser_local_exit_socket(&relay_path)?;
    }
    let mut child = Command::new(path)
        .env(
            "ELASTOS_BROWSER_LOCAL_EXIT_CONFIG",
            serde_json::to_string(config)?,
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            anyhow::bail!("browser-local-exit exited during startup with {status}");
        }
        match browser_local_exit_socket_ready(&relay_path) {
            Ok(true) => return Ok(child),
            Ok(false) => {}
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    anyhow::bail!("browser-local-exit did not create relay socket");
}

fn browser_local_exit_socket_ready(path: &Path) -> anyhow::Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            anyhow::bail!(
                "failed to inspect browser-local-exit relay socket {}: {}",
                path.display(),
                err
            )
        }
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if !metadata.file_type().is_socket() {
            anyhow::bail!(
                "browser-local-exit relay_ipc_path is not a Unix socket: {}",
                path.display()
            );
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
    }

    Ok(true)
}

fn browser_local_exit_relay_path(config: &serde_json::Value) -> anyhow::Result<PathBuf> {
    let relay_path = config
        .get("relay_ipc_path")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("browser-local-exit config missing relay_ipc_path"))?;
    if relay_path.is_empty()
        || relay_path
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte == b'\0')
    {
        anyhow::bail!(
            "browser-local-exit relay_ipc_path must be an absolute path without whitespace"
        );
    }
    let path = PathBuf::from(relay_path);
    if !path.is_absolute() {
        anyhow::bail!(
            "browser-local-exit relay_ipc_path must be an absolute path without whitespace"
        );
    }
    Ok(path)
}

fn remove_existing_browser_local_exit_socket(path: &Path) -> anyhow::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            anyhow::bail!(
                "failed to inspect existing browser-local-exit relay socket {}: {}",
                path.display(),
                err
            )
        }
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if !metadata.file_type().is_socket() {
            anyhow::bail!(
                "refusing to remove browser-local-exit relay_ipc_path because it is not a Unix socket: {}",
                path.display()
            );
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        anyhow::bail!(
            "browser-local-exit replace_existing_socket is only supported for Unix socket paths"
        );
    }

    std::fs::remove_file(path).map_err(|err| {
        anyhow::anyhow!(
            "failed to remove existing browser-local-exit relay socket {}: {}",
            path.display(),
            err
        )
    })
}

fn availability_provider_config_from_env() -> Option<serde_json::Value> {
    if let Ok(raw) = std::env::var("ELASTOS_AVAILABILITY_PROVIDER_CONFIG") {
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(value) => return Some(value),
            Err(err) => {
                tracing::warn!(
                    "Ignoring invalid ELASTOS_AVAILABILITY_PROVIDER_CONFIG JSON: {}",
                    err
                );
                return None;
            }
        }
    }

    let ensure_url = std::env::var("ELASTOS_AVAILABILITY_ENSURE_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let mut target = serde_json::json!({
        "id": std::env::var("ELASTOS_AVAILABILITY_PROVIDER_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "configured-supernode".to_string()),
        "ensure_url": ensure_url,
    });
    if let Ok(value) = std::env::var("ELASTOS_AVAILABILITY_AUTHORIZATION") {
        let value = value.trim();
        if !value.is_empty() {
            target["authorization"] = serde_json::Value::String(value.to_string());
        }
    }

    Some(serde_json::json!({
        "targets": [target]
    }))
}

fn carrier_peer_attestation_exchange_config_from_env() -> Option<serde_json::Value> {
    if let Ok(raw) = std::env::var("ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_CONFIG") {
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(value) => return Some(value),
            Err(err) => {
                tracing::warn!(
                    "Ignoring invalid ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_CONFIG JSON: {}",
                    err
                );
                return None;
            }
        }
    }

    let url = std::env::var("ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let mut config = serde_json::json!({ "url": url });
    if let Ok(value) = std::env::var("ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_AUTHORIZATION") {
        let value = value.trim();
        if !value.is_empty() {
            config["authorization"] = serde_json::Value::String(value.to_string());
        }
    }
    if let Ok(value) = std::env::var("ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_TIMEOUT_SECS") {
        if let Ok(timeout_secs) = value.trim().parse::<u64>() {
            config["timeout_secs"] = serde_json::Value::from(timeout_secs);
        }
    }
    Some(config)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContentRepairSchedulerConfig {
    interval_secs: u64,
    limit: u64,
    max_attempts: u64,
    failure_budget: u64,
    include_healthy_check: bool,
}

fn content_repair_scheduler_config_from_env() -> Option<ContentRepairSchedulerConfig> {
    if !env_flag_enabled(CONTENT_REPAIR_SCHEDULER_ENV) {
        return None;
    }
    Some(ContentRepairSchedulerConfig {
        interval_secs: env_u64(
            CONTENT_REPAIR_SCHEDULER_INTERVAL_ENV,
            CONTENT_REPAIR_SCHEDULER_DEFAULT_INTERVAL_SECS,
        )
        .max(CONTENT_REPAIR_SCHEDULER_MIN_INTERVAL_SECS),
        limit: env_u64(
            CONTENT_REPAIR_SCHEDULER_LIMIT_ENV,
            CONTENT_REPAIR_SCHEDULER_DEFAULT_LIMIT,
        )
        .clamp(1, 100),
        max_attempts: env_u64(
            CONTENT_REPAIR_SCHEDULER_MAX_ATTEMPTS_ENV,
            CONTENT_REPAIR_SCHEDULER_DEFAULT_MAX_ATTEMPTS,
        )
        .clamp(1, 25),
        failure_budget: env_u64(
            CONTENT_REPAIR_SCHEDULER_FAILURE_BUDGET_ENV,
            CONTENT_REPAIR_SCHEDULER_DEFAULT_FAILURE_BUDGET,
        )
        .clamp(1, 100),
        include_healthy_check: env_flag_enabled(CONTENT_REPAIR_SCHEDULER_INCLUDE_HEALTHY_ENV),
    })
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn maybe_spawn_content_repair_scheduler(registry: Arc<provider::ProviderRegistry>) {
    let Some(config) = content_repair_scheduler_config_from_env() else {
        tracing::debug!(
            "{} is disabled; content repair worker remains manual/operator-triggered",
            CONTENT_REPAIR_SCHEDULER_ENV
        );
        return;
    };
    tracing::info!(
        "content repair scheduler enabled: interval={}s limit={} max_attempts={} failure_budget={} include_healthy_check={}",
        config.interval_secs,
        config.limit,
        config.max_attempts,
        config.failure_budget,
        config.include_healthy_check,
    );
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(config.interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match invoke_content_repair_worker(&registry, config).await {
                Ok(response) => {
                    let data = response.get("data").unwrap_or(&response);
                    tracing::debug!(
                        "content repair scheduler run completed: checked={} repaired={} failed={} skipped={}",
                        data.get("checked").and_then(|value| value.as_u64()).unwrap_or(0),
                        data.get("repaired").and_then(|value| value.as_u64()).unwrap_or(0),
                        data.get("failed").and_then(|value| value.as_u64()).unwrap_or(0),
                        data.get("skipped").and_then(|value| value.as_u64()).unwrap_or(0),
                    );
                }
                Err(err) => {
                    tracing::warn!("content repair scheduler run failed: {}", err);
                }
            }
        }
    });
}

async fn invoke_content_repair_worker(
    registry: &provider::ProviderRegistry,
    config: ContentRepairSchedulerConfig,
) -> Result<serde_json::Value, provider::ProviderError> {
    registry
        .invoke_provider(ProviderInvocation {
            source: "content-provider".to_string(),
            target: "content".to_string(),
            op: "repair_worker".to_string(),
            request: serde_json::json!({
                "op": "repair_worker",
                "force": false,
                "include_healthy_check": config.include_healthy_check,
                "limit": config.limit,
                "max_attempts": config.max_attempts,
                "failure_budget": config.failure_budget,
            }),
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Local,
        })
        .await
}

fn exit_provider_config_from_env(data_dir: &std::path::Path) -> Option<serde_json::Value> {
    provider_config_from_env_or_file(
        data_dir,
        "ELASTOS_EXIT_PROVIDER_CONFIG",
        "exit-provider.json",
    )
}

fn browser_engine_adapter_config_from_env(data_dir: &std::path::Path) -> Option<serde_json::Value> {
    provider_config_from_env_or_file(
        data_dir,
        "ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG",
        "browser-engine-adapter.json",
    )
}

fn operator_drive_adapter_config_from_env(data_dir: &std::path::Path) -> Option<serde_json::Value> {
    provider_config_from_env_or_file(
        data_dir,
        "ELASTOS_OPERATOR_DRIVE_ADAPTER_CONFIG",
        "operator-drive-adapter.json",
    )
}

fn browser_local_exit_config_from_env(data_dir: &std::path::Path) -> Option<serde_json::Value> {
    provider_config_from_env_or_file(
        data_dir,
        "ELASTOS_BROWSER_LOCAL_EXIT_CONFIG",
        "browser-local-exit.json",
    )
}

fn provider_config_from_env_or_file(
    data_dir: &std::path::Path,
    env_name: &str,
    file_name: &str,
) -> Option<serde_json::Value> {
    if let Ok(raw) = std::env::var(env_name) {
        return match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(value) => Some(value),
            Err(err) => {
                tracing::warn!("Ignoring invalid {} JSON: {}", env_name, err);
                None
            }
        };
    }
    let path = data_dir.join("config").join(file_name);
    let raw = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(value) => Some(value),
        Err(err) => {
            tracing::warn!(
                "Ignoring invalid provider config {}: {}",
                path.display(),
                err
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // AUD-6 ratchet (build-visible; #[ignore]d = fails today, non-blocking): every boot-critical
    // sub-provider must PROPAGATE a register_sub_provider failure (a spawned-but-unregisterable
    // escrow/keys/signing/mint provider is an invariant violation = a dark path), NOT warn-and-
    // continue. `encrypt` (CEK escrow) is rewired to fail loud; the rest still warn-swallow, so this
    // is ignored until they are classified + rewired. Close the gap = remove the warn lines for the
    // set below, delete `#[ignore]`, the test goes green, and AUD-6 moves to Closed.
    // (Absent-binary stays a warn — genuinely optional — and is NOT scanned here.)
    #[test]
    #[ignore = "AUD-6: only `encrypt` fails loud so far; flip when the rest of the boot-critical spine propagates registration failures"]
    fn aud6_boot_critical_sub_provider_registration_fails_loud() {
        let src = include_str!("server_infra.rs");
        let mut still_swallowing = Vec::new();
        for critical in [
            "encrypt", "publish", "media", "key", "decrypt", "drm", "rights", "wallet", "chain",
        ] {
            let warn = format!("Failed to register elastos://{critical} sub-provider");
            if src.contains(&warn) {
                still_swallowing.push(critical);
            }
        }
        assert!(
            still_swallowing.is_empty(),
            "boot-critical sub-providers still warn-swallow their registration failure (AUD-6, \
             invariant violation should fail boot loud): {still_swallowing:?}"
        );
    }

    struct EnvGuard {
        keys: &'static [&'static str],
    }

    impl EnvGuard {
        fn new(keys: &'static [&'static str]) -> Self {
            for key in keys {
                std::env::remove_var(key);
            }
            Self { keys }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for key in self.keys {
                std::env::remove_var(key);
            }
        }
    }

    #[test]
    fn availability_provider_config_uses_explicit_json() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            "ELASTOS_AVAILABILITY_PROVIDER_CONFIG",
            "ELASTOS_AVAILABILITY_ENSURE_URL",
        ]);
        std::env::set_var(
            "ELASTOS_AVAILABILITY_PROVIDER_CONFIG",
            r#"{"targets":[{"id":"test","ensure_url":"https://example.invalid/ensure"}]}"#,
        );

        let config = availability_provider_config_from_env().unwrap();
        assert_eq!(config["targets"][0]["id"], "test");
    }

    #[test]
    fn availability_provider_config_uses_env_endpoint() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            "ELASTOS_AVAILABILITY_PROVIDER_CONFIG",
            "ELASTOS_AVAILABILITY_ENSURE_URL",
            "ELASTOS_AVAILABILITY_PROVIDER_ID",
            "ELASTOS_AVAILABILITY_AUTHORIZATION",
        ]);
        std::env::set_var(
            "ELASTOS_AVAILABILITY_ENSURE_URL",
            "https://example.invalid/ensure",
        );
        std::env::set_var("ELASTOS_AVAILABILITY_PROVIDER_ID", "elacity");
        std::env::set_var("ELASTOS_AVAILABILITY_AUTHORIZATION", "Bearer secret");

        let config = availability_provider_config_from_env().unwrap();
        assert_eq!(config["targets"][0]["id"], "elacity");
        assert_eq!(
            config["targets"][0]["ensure_url"],
            "https://example.invalid/ensure"
        );
        assert_eq!(config["targets"][0]["authorization"], "Bearer secret");
    }

    #[test]
    fn carrier_peer_attestation_exchange_config_uses_explicit_json() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_CONFIG",
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_URL",
        ]);
        std::env::set_var(
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_CONFIG",
            r#"{"url":"https://attest.example.invalid/exchange","authorization":"Bearer attest","timeout_secs":8}"#,
        );

        let config = carrier_peer_attestation_exchange_config_from_env().unwrap();
        assert_eq!(config["url"], "https://attest.example.invalid/exchange");
        assert_eq!(config["authorization"], "Bearer attest");
        assert_eq!(config["timeout_secs"], 8);
    }

    #[test]
    fn carrier_peer_attestation_exchange_config_uses_env_endpoint() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_CONFIG",
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_URL",
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_AUTHORIZATION",
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_TIMEOUT_SECS",
        ]);
        std::env::set_var(
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_URL",
            "http://127.0.0.1:9799/peer-attestation/exchange",
        );
        std::env::set_var(
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_AUTHORIZATION",
            "Bearer local-attest",
        );
        std::env::set_var(
            "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_TIMEOUT_SECS",
            "10",
        );

        let config = carrier_peer_attestation_exchange_config_from_env().unwrap();
        assert_eq!(
            config["url"],
            "http://127.0.0.1:9799/peer-attestation/exchange"
        );
        assert_eq!(config["authorization"], "Bearer local-attest");
        assert_eq!(config["timeout_secs"], 10);
    }

    #[test]
    fn content_repair_scheduler_is_opt_in() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            CONTENT_REPAIR_SCHEDULER_ENV,
            CONTENT_REPAIR_SCHEDULER_INTERVAL_ENV,
            CONTENT_REPAIR_SCHEDULER_LIMIT_ENV,
            CONTENT_REPAIR_SCHEDULER_MAX_ATTEMPTS_ENV,
            CONTENT_REPAIR_SCHEDULER_FAILURE_BUDGET_ENV,
            CONTENT_REPAIR_SCHEDULER_INCLUDE_HEALTHY_ENV,
        ]);

        assert!(content_repair_scheduler_config_from_env().is_none());
    }

    #[test]
    fn content_repair_scheduler_config_clamps_operator_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            CONTENT_REPAIR_SCHEDULER_ENV,
            CONTENT_REPAIR_SCHEDULER_INTERVAL_ENV,
            CONTENT_REPAIR_SCHEDULER_LIMIT_ENV,
            CONTENT_REPAIR_SCHEDULER_MAX_ATTEMPTS_ENV,
            CONTENT_REPAIR_SCHEDULER_FAILURE_BUDGET_ENV,
            CONTENT_REPAIR_SCHEDULER_INCLUDE_HEALTHY_ENV,
        ]);
        std::env::set_var(CONTENT_REPAIR_SCHEDULER_ENV, "true");
        std::env::set_var(CONTENT_REPAIR_SCHEDULER_INTERVAL_ENV, "5");
        std::env::set_var(CONTENT_REPAIR_SCHEDULER_LIMIT_ENV, "1000");
        std::env::set_var(CONTENT_REPAIR_SCHEDULER_MAX_ATTEMPTS_ENV, "999");
        std::env::set_var(CONTENT_REPAIR_SCHEDULER_FAILURE_BUDGET_ENV, "999");
        std::env::set_var(CONTENT_REPAIR_SCHEDULER_INCLUDE_HEALTHY_ENV, "yes");

        let config = content_repair_scheduler_config_from_env().unwrap();
        assert_eq!(
            config.interval_secs,
            CONTENT_REPAIR_SCHEDULER_MIN_INTERVAL_SECS
        );
        assert_eq!(config.limit, 100);
        assert_eq!(config.max_attempts, 25);
        assert_eq!(config.failure_budget, 100);
        assert!(config.include_healthy_check);
    }

    #[test]
    fn exit_provider_config_prefers_explicit_json() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&["ELASTOS_EXIT_PROVIDER_CONFIG"]);
        std::env::set_var(
            "ELASTOS_EXIT_PROVIDER_CONFIG",
            r#"{"backends":[{"id":"local","kind":"http_fetch","allowed_hosts":["example.com"]}]}"#,
        );

        let data_dir = crate::sources::default_data_dir();
        let config = exit_provider_config_from_env(&data_dir).unwrap();
        assert_eq!(config["backends"][0]["id"], "local");
        assert_eq!(config["backends"][0]["kind"], "http_fetch");
    }

    #[test]
    fn browser_engine_adapter_config_prefers_explicit_json() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&["ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG"]);
        std::env::set_var(
            "ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG",
            r#"{"adapters":[{"id":"linux-proof","kind":"contract_proof","display_modes":["webrtc_remote_display"]}]}"#,
        );

        let data_dir = crate::sources::default_data_dir();
        let config = browser_engine_adapter_config_from_env(&data_dir).unwrap();
        assert_eq!(config["adapters"][0]["id"], "linux-proof");
        assert_eq!(config["adapters"][0]["kind"], "contract_proof");
        assert_eq!(
            config["adapters"][0]["display_modes"][0],
            "webrtc_remote_display"
        );
    }

    #[test]
    fn operator_drive_adapter_config_prefers_explicit_json() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&["ELASTOS_OPERATOR_DRIVE_ADAPTER_CONFIG"]);
        std::env::set_var(
            "ELASTOS_OPERATOR_DRIVE_ADAPTER_CONFIG",
            r#"{"operator_endpoint":{"url":"http://127.0.0.1:9797/operator-drive","authorization":"Bearer secret"}}"#,
        );

        let data_dir = crate::sources::default_data_dir();
        let config = operator_drive_adapter_config_from_env(&data_dir).unwrap();
        assert_eq!(
            config["operator_endpoint"]["url"],
            "http://127.0.0.1:9797/operator-drive"
        );
        assert_eq!(
            config["operator_endpoint"]["authorization"],
            "Bearer secret"
        );
    }

    #[test]
    fn browser_local_exit_config_prefers_explicit_json() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&["ELASTOS_BROWSER_LOCAL_EXIT_CONFIG"]);
        std::env::set_var(
            "ELASTOS_BROWSER_LOCAL_EXIT_CONFIG",
            r#"{"schema":"elastos.browser.local-exit.config/v1","relay_ipc_path":"/tmp/elastos-browser-local-exit.sock","allowed_hosts":["*"],"replace_existing_socket":true}"#,
        );

        let data_dir = crate::sources::default_data_dir();
        let config = browser_local_exit_config_from_env(&data_dir).unwrap();
        assert_eq!(config["schema"], "elastos.browser.local-exit.config/v1");
        assert_eq!(config["allowed_hosts"][0], "*");
    }

    #[test]
    fn browser_local_exit_relay_path_rejects_relative_and_whitespace_paths() {
        let relative = serde_json::json!({
            "relay_ipc_path": "relative.sock"
        });
        assert!(browser_local_exit_relay_path(&relative).is_err());

        let whitespace = serde_json::json!({
            "relay_ipc_path": "/tmp/elastos browser.sock"
        });
        assert!(browser_local_exit_relay_path(&whitespace).is_err());
    }

    #[test]
    fn browser_local_exit_replace_existing_socket_refuses_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let relay_path = dir.path().join("relay.sock");
        std::fs::write(&relay_path, b"not a socket").unwrap();

        let err = remove_existing_browser_local_exit_socket(&relay_path).unwrap_err();
        assert!(
            err.to_string().contains("not a Unix socket")
                || err
                    .to_string()
                    .contains("replace_existing_socket is only supported")
        );
        assert!(relay_path.exists());
    }

    #[test]
    fn browser_local_exit_socket_ready_requires_socket() {
        let dir = tempfile::tempdir().unwrap();
        let relay_path = dir.path().join("relay.sock");
        assert!(!browser_local_exit_socket_ready(&relay_path).unwrap());
        std::fs::write(&relay_path, b"not a socket").unwrap();
        #[cfg(unix)]
        {
            let err = browser_local_exit_socket_ready(&relay_path).unwrap_err();
            assert!(err.to_string().contains("not a Unix socket"), "{err}");
        }
        #[cfg(not(unix))]
        {
            assert!(browser_local_exit_socket_ready(&relay_path).unwrap());
        }
    }
}
