use std::path::PathBuf;
use std::sync::Arc;

use elastos_compute::ComputeProvider;
use elastos_crosvm::{CrosvmConfig, CrosvmProvider};
use elastos_runtime::{bootstrap, session};

pub async fn run_serve(
    addr: String,
    storage_path: PathBuf,
    capsule: Option<PathBuf>,
    cid: Option<String>,
) -> anyhow::Result<()> {
    let data_dir = crate::default_data_dir();
    let (_runtime_config, is_first_run) = bootstrap::RuntimeConfig::load(&data_dir);
    if is_first_run {
        crate::print_first_run_welcome(&data_dir);
    }

    eprintln!(
        "ElastOS Runtime v{} starting on {}",
        crate::ELASTOS_VERSION,
        addr
    );
    tracing::info!(
        "Starting ElastOS Runtime server v{} on {}",
        crate::ELASTOS_VERSION,
        addr
    );

    let runtime = crate::create_runtime(&storage_path).await?;

    let capsule_dir = if let Some(ref cid_str) = cid {
        tracing::info!("Loading capsule from CID: {}", cid_str);
        let ipfs_bridge = crate::get_ipfs_bridge().await?;
        Some(elastos_server::ipfs::prepare_capsule_from_cid(&ipfs_bridge, cid_str).await?)
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
                tracing::info!("Launching MicroVM capsule: {}", manifest.name);

                let vm_provider = CrosvmProvider::new(CrosvmConfig::default())
                    .map_err(|e| anyhow::anyhow!("Failed to create crosvm provider: {}", e))?;
                vm_provider
                    .init()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to init crosvm provider: {}", e))?;

                let infra = crate::setup_server_infrastructure().await?;
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

                    tracing::info!(
                        "Created session for VM {}: token={}... tap={}",
                        vm_id,
                        &shell_session.token[..8],
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
                    tracing::warn!(
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
                tracing::info!("API server will bind to {}", api_bind_addr);

                let provider_registry = infra.provider_registry;
                let namespace_store = infra.namespace_store;
                let identity_state = infra.identity_state;

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
                                bootstrap_state: None,
                                tls_config,
                                supervisor: None,
                                ready_tx: None,
                                attach_secret: None,
                            },
                        )
                        .await
                        {
                            tracing::error!("API server error: {}", e);
                        }
                    }
                });

                tokio::signal::ctrl_c().await?;
                println!("\nStopping...");
                api_handle.abort();
                runtime_arc.unregister_capsule(&handle.id.0).await;
                if let Err(e) = vm_provider.stop(&handle).await {
                    tracing::warn!("Error stopping VM: {}", e);
                }
                println!("MicroVM stopped.");
                return Ok(());
            }

            if manifest.capsule_type == elastos_common::CapsuleType::Wasm {
                let infra = crate::setup_server_infrastructure().await?;
                runtime
                    .set_provider_registry(
                        infra.provider_registry.clone(),
                        infra.capability_manager.clone(),
                        infra.pending_store.clone(),
                    )
                    .await;

                eprintln!(
                    "[serve] WASM capsule '{}' with Carrier bridge active",
                    manifest.name
                );

                let handle = runtime
                    .run_local(&capsule_dir, vec![])
                    .await
                    .map_err(|e| anyhow::anyhow!("WASM capsule failed: {}", e))?;

                eprintln!("[serve] WASM capsule '{}' exited", handle.manifest.name);
                return Ok(());
            }
        }

        tracing::info!("Serving web capsule from: {}", capsule_dir.display());
        return crate::serve_web_capsule(runtime, capsule_dir, &addr, false, None).await;
    }

    let infra = crate::setup_server_infrastructure().await?;
    runtime
        .set_provider_registry(
            infra.provider_registry.clone(),
            infra.capability_manager.clone(),
            infra.pending_store.clone(),
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
            tracing::warn!("Skipping shell capsule due to verification failure: {}", e);
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
                    tracing::info!(
                        "Spawned shell capsule (PID {}, mode={})",
                        child.id().unwrap_or(0),
                        shell_mode
                    );
                    Some(child)
                }
                Err(e) => {
                    tracing::warn!("Failed to spawn shell capsule: {}", e);
                    None
                }
            }
        }
    } else {
        tracing::info!("Shell capsule not found, skipping spawn");
        None
    };

    println!("ElastOS Runtime");
    println!("  Capsule  localhost-provider  {}", infra.provider_cid);
    if let Some(ref cid) = infra.shell_cid {
        println!("  Capsule  shell           {}", cid);
    }
    if let Some(ref cid) = infra.notepad_cid {
        println!("  Capsule  notepad         {}", cid);
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
        .unwrap_or_else(|| crate::shell_cmd::RUNTIME_KIND_OPERATOR.to_string());
    let coords = crate::shell_cmd::RuntimeCoords {
        api_url: format!(
            "http://127.0.0.1:{}",
            addr.rsplit(':').next().unwrap_or("3000")
        ),
        attach_secret: attach_secret.clone(),
        shell_token: String::new(),
        client_token: String::new(),
        pid: std::process::id(),
        runtime_kind: runtime_kind.clone(),
    };
    let coords_path = crate::shell_cmd::runtime_coord_path(&data_dir);
    if let Err(e) = crate::shell_cmd::write_runtime_coords(&coords_path, &coords) {
        eprintln!("[serve] Warning: failed to write runtime coords: {}", e);
    } else if runtime_kind == crate::shell_cmd::RUNTIME_KIND_MANAGED_CHAT {
        eprintln!("[serve] Managed chat runtime ready");
    } else {
        eprintln!("[serve] Attach commands (elastos chat, elastos run) ready");
    }

    {
        let registry = infra.session_registry.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                let removed = registry.cleanup_stale_sessions(600).await;
                if removed > 0 {
                    tracing::debug!("Cleaned up {} idle attach sessions", removed);
                }
            }
        });
    }

    elastos_server::api::server::start_server_with_sessions(
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
            data_dir: None,
            bootstrap_state: None,
            tls_config: None,
            supervisor: sup,
            ready_tx: None,
            attach_secret: Some(attach_secret),
        },
    )
    .await?;

    Ok(())
}
