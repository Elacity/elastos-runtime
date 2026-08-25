use std::path::PathBuf;
use std::sync::Arc;

use elastos_compute::ComputeProvider;
use elastos_crosvm::{CrosvmConfig, CrosvmProvider};
use elastos_logger::{fp, log_error, log_info, log_trace, log_warn};
use elastos_runtime::{bootstrap, session};
use sha2::Digest as _;

const LOG_COMPONENT: &str = "serve";

pub async fn run_serve(
    addr: String,
    storage_path: PathBuf,
    capsule: Option<PathBuf>,
    cid: Option<String>,
) -> anyhow::Result<()> {
    let data_dir = crate::default_data_dir();
    let subordinate_host = std::env::var("ELASTOS_ALLOW_SUBORDINATE_RUNTIME_HOST")
        .ok()
        .as_deref()
        == Some("1");
    let _host_guard = if subordinate_host {
        None
    } else {
        Some(elastos_server::host_lock::acquire_host_process_lock(
            &data_dir, "serve", &addr,
        )?)
    };
    if !subordinate_host {
        elastos_server::host_lock::spawn_installed_binary_supersession_watch(&data_dir, "serve");
    }
    let (_runtime_config, is_first_run) = bootstrap::RuntimeConfig::load(&data_dir);
    if is_first_run {
        crate::print_first_run_welcome(&data_dir);
    }

    log_info!(
        component: LOG_COMPONENT,
        "ElastOS Runtime v{} starting on {}",
        crate::ELASTOS_VERSION,
        addr
    );
    log_info!(
        component: LOG_COMPONENT,
        "Starting ElastOS Runtime server v{} on {}",
        crate::ELASTOS_VERSION,
        addr
    );

    let runtime = crate::create_runtime(&storage_path).await?;

    let capsule_dir = if let Some(ref cid_str) = cid {
        log_info!(component: LOG_COMPONENT, "Loading capsule from CID: {}", cid_str);
        let content_registry = crate::get_content_registry().await?;
        Some(
            elastos_server::content::prepare_capsule_from_content_provider(
                &content_registry,
                cid_str,
            )
            .await?,
        )
    } else {
        capsule.clone()
    };

    if let Some(capsule_dir) = capsule_dir {
        let manifest_path = capsule_dir.join("capsule.json");
        if manifest_path.exists() {
            let manifest_data = tokio::fs::read_to_string(&manifest_path).await?;
            let manifest: elastos_common::CapsuleManifest = serde_json::from_str(&manifest_data)?;
            manifest
                .validate()
                .map_err(|e| anyhow::anyhow!("Invalid manifest: {}", e))?;

            if manifest.capsule_type == elastos_common::CapsuleType::MicroVM {
                log_info!(component: LOG_COMPONENT, "Launching MicroVM capsule: {}", manifest.name);

                let vm_provider = CrosvmProvider::new(CrosvmConfig::default())
                    .map_err(|e| anyhow::anyhow!("Failed to create crosvm provider: {}", e))?;
                vm_provider
                    .init()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to init crosvm provider: {}", e))?;

                let infra = crate::setup_server_infrastructure().await?;
                let mut collaboration_service = infra.collaboration_service;
                let mut carrier_service = infra.carrier_service;
                let audit_log = infra.audit_log.clone();
                let session_registry = infra.session_registry.clone();
                let capability_manager = infra.capability_manager.clone();
                let pending_store = infra.pending_store.clone();
                let tls_config = infra.tls_config;

                let handle = vm_provider
                    .load(&capsule_dir, manifest.clone())
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to load capsule: {}", e))?;

                if let Some(vm_id) = vm_provider.get_vm_id(&handle.id).await {
                    let shell_session = session_registry
                        .create_session(session::SessionType::Shell, Some(vm_id.clone()))
                        .await;

                    let needs_tap = manifest.permissions.guest_network;
                    if needs_tap {
                        let network = elastos_crosvm::NetworkConfig::new(&vm_id);
                        vm_provider
                            .set_network_for_vm(&handle.id, network.clone())
                            .await
                            .map_err(|e| {
                                anyhow::anyhow!("Failed to configure guest network: {}", e)
                            })?;
                    }

                    if needs_tap {
                        let api_port = addr
                            .rsplit(':')
                            .next()
                            .and_then(|p| p.parse::<u16>().ok())
                            .ok_or_else(|| anyhow::anyhow!("Invalid serve address: {}", addr))?;
                        let net = vm_provider
                            .get_network_for_vm(&handle.id)
                            .await
                            .ok_or_else(|| anyhow::anyhow!("network not configured"))?;
                        let api_addr = format!("http://{}:{}", net.host_ip, api_port);
                        vm_provider
                            .set_session_for_vm(&handle.id, &shell_session.token, &api_addr)
                            .await
                            .map_err(|e| anyhow::anyhow!("Failed to set session: {}", e))?;
                    } else {
                        vm_provider
                            .append_boot_args_for_vm(
                                &handle.id,
                                &format!("elastos.token={}", shell_session.token),
                            )
                            .await
                            .map_err(|e| anyhow::anyhow!("Failed to set token: {}", e))?;
                    }

                    log_info!(
                        component: LOG_COMPONENT,
                        "Created session for VM {}: token={}... tap={}",
                        vm_id,
                        fp(&shell_session.token),
                        needs_tap,
                    );
                }

                vm_provider
                    .start(&handle)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to start VM: {}", e))?;

                let vm_port = handle
                    .manifest
                    .microvm
                    .as_ref()
                    .and_then(|m| m.http_port)
                    .unwrap_or(4100);

                let runtime_arc = Arc::new(runtime);
                let capsule_info = elastos_server::runtime::RunningCapsuleInfo {
                    id: handle.id.0.clone(),
                    name: handle.manifest.name.clone(),
                    status: "running".to_string(),
                    capsule_type: handle.manifest.capsule_type.clone(),
                    handle: Some(handle.clone()),
                };
                runtime_arc.register_capsule(capsule_info).await;

                if tls_config.is_some() {
                    log_warn!(
                        component: LOG_COMPONENT,
                        "TLS proxy not available on the current VM path. Using plain HTTP."
                    );
                }

                let scheme = if tls_config.is_some() {
                    "https"
                } else {
                    "http"
                };
                println!("MicroVM capsule '{}' started", handle.manifest.name);
                println!("  API server: {}://{}", scheme, addr);
                println!("  VM service: {}://localhost:{}", scheme, vm_port);
                println!("  Session: configured (shell mode)");
                println!("Press Ctrl+C to stop...");

                let api_bind_addr = if addr.starts_with("127.0.0.1:") {
                    addr.replacen("127.0.0.1", "0.0.0.0", 1)
                } else if addr.starts_with("localhost:") {
                    addr.replacen("localhost", "0.0.0.0", 1)
                } else {
                    addr.clone()
                };
                log_info!(component: LOG_COMPONENT, "API server will bind to {}", api_bind_addr);

                let provider_registry = infra.provider_registry;
                let namespace_store = infra.namespace_store;
                let identity_state = infra.identity_state;
                let host_helpers = infra.host_helpers;

                let api_handle = tokio::spawn({
                    let runtime = runtime_arc.clone();
                    let session_registry = session_registry.clone();
                    let capability_manager = capability_manager.clone();
                    let pending_store = pending_store.clone();
                    async move {
                        if let Err(e) = elastos_server::api::server::start_server_with_sessions(
                            elastos_server::api::server::ServerConfig {
                                runtime,
                                session_registry,
                                capability_manager,
                                pending_store,
                                namespace_store: Some(namespace_store),
                                provider_registry: Some(provider_registry),
                                audit_log: Some(audit_log),
                                identity_state,
                                docs_dir: std::env::current_dir().ok().and_then(|d| {
                                    let docs = d.join("..");
                                    if docs.join("ROADMAP.md").exists() {
                                        Some(docs)
                                    } else {
                                        None
                                    }
                                }),
                                addr: api_bind_addr,
                                capsule_dir: None,
                                data_dir: None,
                                tls_config,
                                supervisor: None,
                                ready_tx: None,
                                attach_secret: None,
                                host_helpers,
                            },
                        )
                        .await
                        {
                            log_error!(component: LOG_COMPONENT, "API server error: {}", e);
                        }
                    }
                });

                tokio::signal::ctrl_c().await?;
                println!("\nStopping...");
                api_handle.abort();
                runtime_arc.unregister_capsule(&handle.id.0).await;
                if let Err(e) = vm_provider.stop(&handle).await {
                    log_warn!(component: LOG_COMPONENT, "Error stopping VM: {}", e);
                }
                if let Some(service) = collaboration_service.as_mut() {
                    service.shutdown().await?;
                }
                if let Some(service) = carrier_service.as_mut() {
                    service.shutdown().await?;
                }
                println!("MicroVM stopped.");
                return Ok(());
            }

            if manifest.capsule_type == elastos_common::CapsuleType::Wasm {
                let infra = crate::setup_server_infrastructure().await?;
                let mut collaboration_service = infra.collaboration_service;
                let mut carrier_service = infra.carrier_service;
                runtime
                    .set_provider_registry(
                        infra.provider_registry.clone(),
                        infra.capability_manager.clone(),
                        infra.pending_store.clone(),
                        data_dir.clone(),
                    )
                    .await;

                log_info!(
                    component: LOG_COMPONENT,
                    "WASM capsule '{}' with resource bridge active",
                    manifest.name
                );

                let run_result = runtime.run_local(&capsule_dir, vec![]).await;
                if let Some(service) = collaboration_service.as_mut() {
                    service.shutdown().await?;
                }
                if let Some(service) = carrier_service.as_mut() {
                    service.shutdown().await?;
                }
                let handle =
                    run_result.map_err(|e| anyhow::anyhow!("WASM capsule failed: {}", e))?;

                log_info!(component: LOG_COMPONENT, "WASM capsule '{}' exited", handle.manifest.name);
                return Ok(());
            }
        }

        log_info!(component: LOG_COMPONENT, "Serving web capsule from: {}", capsule_dir.display());
        return crate::serve_web_capsule(runtime, capsule_dir, &addr, false, None).await;
    }

    let infra = crate::setup_server_infrastructure().await?;
    let collaboration_context = infra.collaboration_context.clone();
    let mut collaboration_service = infra.collaboration_service;
    let mut carrier_service = infra.carrier_service;
    runtime
        .set_provider_registry(
            infra.provider_registry.clone(),
            infra.capability_manager.clone(),
            infra.pending_store.clone(),
            data_dir.clone(),
        )
        .await;

    let runtime = Arc::new(runtime);
    let docs_dir = std::env::current_dir().ok().and_then(|d| {
        let docs = d.join("..");
        if docs.join("ROADMAP.md").exists() {
            Some(docs)
        } else {
            None
        }
    });

    let shell_session = infra
        .session_registry
        .create_session(session::SessionType::Shell, None)
        .await;
    let app_session = infra
        .session_registry
        .create_session(session::SessionType::Capsule, None)
        .await;

    let _shell_child = if let Some(shell_path) = crate::find_installed_provider_binary("shell") {
        if let Err(e) = crate::verify_component_binary("shell", &shell_path) {
            log_warn!(
                component: LOG_COMPONENT,
                "Skipping shell capsule due to verification failure: {}",
                e
            );
            None
        } else {
            let api_url = format!("http://{}", addr);
            let shell_mode = std::env::var("ELASTOS_SHELL_MODE").unwrap_or_else(|_| "auto".into());
            let stdin_cfg = if shell_mode == "cli" {
                std::process::Stdio::inherit()
            } else {
                std::process::Stdio::piped()
            };
            match tokio::process::Command::new(&shell_path)
                .env("ELASTOS_API", &api_url)
                .env("ELASTOS_TOKEN", &shell_session.token)
                .env("ELASTOS_SHELL_MODE", &shell_mode)
                .stdin(stdin_cfg)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
            {
                Ok(child) => {
                    log_info!(
                        component: LOG_COMPONENT,
                        "Spawned shell capsule (PID {}, mode={})",
                        child.id().unwrap_or(0),
                        shell_mode
                    );
                    Some(child)
                }
                Err(e) => {
                    log_warn!(component: LOG_COMPONENT, "Failed to spawn shell capsule: {}", e);
                    None
                }
            }
        }
    } else {
        log_info!(component: LOG_COMPONENT, "Shell capsule not found, skipping spawn");
        None
    };

    println!("ElastOS Runtime");
    println!("  Capsule  localhost-provider  {}", infra.provider_cid);
    if let Some(ref cid) = infra.shell_cid {
        println!("  Capsule  shell           {}", cid);
    }
    println!("  App:     {}", app_session.token);
    println!("  API:     http://{}", addr);

    let components_path = data_dir.join("components.json");
    let sup = if components_path.exists() {
        let components_data = tokio::fs::read_to_string(&components_path).await?;
        if let Ok(registry) =
            serde_json::from_str::<elastos_server::setup::ComponentsManifest>(&components_data)
        {
            let mut s = elastos_server::supervisor::Supervisor::new(data_dir.clone(), registry);
            s.set_session(
                shell_session.token.clone(),
                addr.clone(),
                infra.session_registry.clone(),
            );
            s.set_provider_registry(infra.provider_registry.clone());
            if let Some(port) = collaboration_context.chat_product_port.clone() {
                s.set_collaboration_chat_product_port(port);
            }
            if let Some(port) = collaboration_context.presence_product_port.clone() {
                s.set_collaboration_presence_product_port(port);
            }
            if let Some(service) = collaboration_context.discovery_service.clone() {
                s.set_collaboration_discovery_service(service);
            }
            s.set_capability_manager(infra.capability_manager.clone());
            s.set_pending_store(infra.pending_store.clone());
            Some(Arc::new(s))
        } else {
            None
        }
    } else {
        None
    };

    let attach_secret = {
        let mut buf = [0u8; 32];
        getrandom::getrandom(&mut buf).expect("getrandom failed");
        hex::encode(buf)
    };
    let runtime_kind = std::env::var("ELASTOS_RUNTIME_KIND")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| crate::runtime_control::RUNTIME_KIND_OPERATOR.to_string());
    let binary_sha256 = match std::env::var("ELASTOS_RUNTIME_BINARY_SHA256") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => current_binary_sha256().unwrap_or_default(),
    };
    let policy_sha256 = std::env::var("ELASTOS_RUNTIME_POLICY_SHA256").unwrap_or_default();
    let dependency_sha256 = std::env::var("ELASTOS_RUNTIME_DEPENDENCY_SHA256").unwrap_or_default();
    let coords = crate::runtime_control::RuntimeCoords {
        api_url: format!(
            "http://127.0.0.1:{}",
            addr.rsplit(':').next().unwrap_or("3000")
        ),
        attach_secret: attach_secret.clone(),
        pid: std::process::id(),
        runtime_kind: runtime_kind.clone(),
        binary_sha256,
        policy_sha256,
        dependency_sha256,
    };
    let coords_path = crate::runtime_control::runtime_coord_path(&data_dir);
    if let Err(e) = crate::runtime_control::write_runtime_coords(&coords_path, &coords) {
        log_warn!(component: LOG_COMPONENT, "failed to write runtime coords: {}", e);
    } else if runtime_kind == crate::runtime_control::RUNTIME_KIND_MANAGED_CHAT {
        log_info!(component: LOG_COMPONENT, "Managed chat runtime ready");
    } else {
        log_info!(component: LOG_COMPONENT, "Attach commands (elastos chat, elastos run) ready");
    }

    {
        let registry = infra.session_registry.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                let removed = registry.cleanup_stale_sessions(600).await;
                if removed > 0 {
                    log_trace!(component: LOG_COMPONENT, "Cleaned up {} idle attach sessions", removed);
                }
            }
        });
    }

    let server_result = elastos_server::api::server::start_server_with_sessions(
        elastos_server::api::server::ServerConfig {
            runtime,
            session_registry: infra.session_registry,
            capability_manager: infra.capability_manager,
            pending_store: infra.pending_store,
            namespace_store: Some(infra.namespace_store),
            provider_registry: Some(infra.provider_registry),
            audit_log: Some(infra.audit_log),
            identity_state: infra.identity_state,
            docs_dir,
            addr,
            capsule_dir: None,
            data_dir: Some(data_dir.clone()),
            tls_config: None,
            supervisor: sup,
            ready_tx: None,
            attach_secret: Some(attach_secret),
            host_helpers: infra.host_helpers,
        },
    )
    .await;
    let collaboration_shutdown = async {
        if let Some(service) = collaboration_service.as_mut() {
            service.shutdown().await?;
        }
        if let Some(service) = carrier_service.as_mut() {
            service.shutdown().await?;
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    server_result?;
    collaboration_shutdown?;

    Ok(())
}

fn current_binary_sha256() -> anyhow::Result<String> {
    let self_exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Failed to determine runtime binary: {}", e))?;
    let bytes = std::fs::read(self_exe)?;
    Ok(format!("{:x}", sha2::Sha256::digest(bytes)))
}
