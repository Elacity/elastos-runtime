use std::path::PathBuf;
use std::sync::Arc;

use elastos_compute::ComputeProvider;
use elastos_crosvm::{CrosvmConfig, CrosvmProvider};
use elastos_runtime::{bootstrap, session};
use sha2::Digest as _;

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
    let (runtime_config, is_first_run) = bootstrap::RuntimeConfig::load(&data_dir);
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
                    manifest: Box::new(handle.manifest.clone()),
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
                let host_helpers = infra.host_helpers;

                // Register the read-only Capsule Inspector on the shared
                // provider registry — the one reached by both the API/gateway
                // and the capsule carrier bridge. Its data source is this
                // runtime's running-capsule registry.
                {
                    use elastos_server::inspect_provider as ip;
                    let source: Arc<dyn ip::InspectSource> =
                        Arc::new(ip::RuntimeInspectSource::new(Arc::downgrade(&runtime_arc)));
                    // G1b-LIVE: compose signed activity (AuthAuditSource) with observed
                    // grants from the SAME AuditLog the capability manager records to,
                    // so a running capsule's grants surface live.
                    let activity = Arc::new(ip::AuthAuditSource::new(data_dir.clone()));
                    let grants = Arc::new(ip::RuntimeAuditLogGrantSource::new(
                        capability_manager.audit_log().clone(),
                    ));
                    let audit = Arc::new(ip::CompositeAuditSource::new(activity, grants));
                    provider_registry
                        .register(Arc::new(ip::InspectProvider::new(source).with_audit(audit)))
                        .await;
                }

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
                                host_helpers,
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
                        data_dir.clone(),
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
            data_dir.clone(),
        )
        .await;

    let runtime = Arc::new(runtime);

    // Register the read-only Capsule Inspector on the shared provider registry
    // (the same Arc handed to the supervisor/gateway and the carrier bridge).
    // The source aggregates this runtime's running capsules with the rich
    // installed-capsule catalog (<data_dir>/capsules/<name>/capsule.json), so
    // the browser Inspector shows the full manifest-backed view of what the
    // product knows.
    {
        use elastos_server::inspect_provider as ip;
        let runtime_src: Arc<dyn ip::InspectSource> =
            Arc::new(ip::RuntimeInspectSource::new(Arc::downgrade(&runtime)));
        let catalog_src: Arc<dyn ip::InspectSource> = Arc::new(ip::CatalogInspectSource::new(
            data_dir.join("capsules"),
            Arc::downgrade(&infra.provider_registry),
        ));
        let source: Arc<dyn ip::InspectSource> = Arc::new(ip::AggregateInspectSource::new(vec![
            runtime_src,
            catalog_src,
        ]));
        // G1b-LIVE: compose signed activity with observed grants from the SAME
        // AuditLog the capability manager records to, so live grants surface.
        let activity = Arc::new(ip::AuthAuditSource::new(data_dir.clone()));
        let grants = Arc::new(ip::RuntimeAuditLogGrantSource::new(
            infra.capability_manager.audit_log().clone(),
        ));
        let audit = Arc::new(ip::CompositeAuditSource::new(activity, grants));
        infra
            .provider_registry
            .register(Arc::new(ip::InspectProvider::new(source).with_audit(audit)))
            .await;
    }

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
            // AUD-1: seed the author-signature launch gate from config `trusted_keys`.
            // Empty by default (gate inert, launches byte-for-byte unchanged); a
            // malformed hex key aborts serve startup LOUDLY (fail-closed at boot) rather
            // than dropping it and leaving a partial/empty keyset that fails open.
            let mut verifier = elastos_runtime::signature::SignatureVerifier::new();
            for key_hex in runtime_config.effective_trusted_keys() {
                verifier.add_trusted_key_hex(&key_hex).map_err(|e| {
                    anyhow::anyhow!(
                        "invalid trusted_keys entry in runtime config (refusing to start): {e}"
                    )
                })?;
            }
            s.set_signature_verifier(verifier);
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
    };
    let coords_path = crate::runtime_control::runtime_coord_path(&data_dir);
    if let Err(e) = crate::runtime_control::write_runtime_coords(&coords_path, &coords) {
        eprintln!("[serve] Warning: failed to write runtime coords: {}", e);
    } else if runtime_kind == crate::runtime_control::RUNTIME_KIND_MANAGED_CHAT {
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
            data_dir: Some(data_dir.clone()),
            bootstrap_state: None,
            tls_config: None,
            supervisor: sup,
            ready_tx: None,
            attach_secret: Some(attach_secret),
            host_helpers: infra.host_helpers,
        },
    )
    .await?;

    Ok(())
}

fn current_binary_sha256() -> anyhow::Result<String> {
    let self_exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Failed to determine runtime binary: {}", e))?;
    let bytes = std::fs::read(self_exe)?;
    Ok(format!("{:x}", sha2::Sha256::digest(bytes)))
}

/// `elastos mcp serve` — the stdio I/O shell for the read-only MCP bridge. Builds the
/// inspect infra, mints the scoped bridge token, and pumps newline-delimited JSON-RPC
/// over stdin/stdout, driving the gated core (`mcp_serve_cmd::handle_mcp_message`). The
/// MCP client (Claude Code / Codex / Gemini) spawns this as a child process; all logging
/// goes to stderr, stdout carries ONLY MCP messages.
pub async fn run_mcp_serve() -> anyhow::Result<()> {
    use elastos_server::carrier_bridge::BridgeContext;
    use elastos_server::inspect_provider as ip;
    use elastos_server::mcp_serve_cmd::{handle_mcp_message, mint_bridge_token, MCP_BRIDGE_ID};
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let data_dir = crate::default_data_dir();
    let infra = crate::setup_server_infrastructure().await?;

    // Read-only Capsule Inspector over the installed-capsule catalog, with the live
    // grant/activity audit (mirrors serve_cmd's inspect wiring, minus the running runtime).
    {
        let source: Arc<dyn ip::InspectSource> = Arc::new(ip::CatalogInspectSource::new(
            data_dir.join("capsules"),
            Arc::downgrade(&infra.provider_registry),
        ));
        let activity = Arc::new(ip::AuthAuditSource::new(data_dir.clone()));
        let grants = Arc::new(ip::RuntimeAuditLogGrantSource::new(
            infra.capability_manager.audit_log().clone(),
        ));
        let audit = Arc::new(ip::CompositeAuditSource::new(activity, grants));
        infra
            .provider_registry
            .register(Arc::new(ip::InspectProvider::new(source).with_audit(audit)))
            .await;
    }

    let token = mint_bridge_token(&infra.capability_manager);
    let ctx = Some(BridgeContext {
        provider_registry: infra.provider_registry.clone(),
        capability_manager: infra.capability_manager.clone(),
        pending_store: infra.pending_store.clone(),
        capsule_id: MCP_BRIDGE_ID.to_string(),
        principal_id: None,
        data_dir: None,
        // Act-over-MCP spend metering (enabled when ELASTOS_DEFAULT_SPEND_BUDGET is set).
        spend_policy: infra.spend_policy.clone(),
    });

    eprintln!("elastos mcp serve: ready (stdio, read-only inspect; operator-authority).");
    let mut reader = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = reader.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: serde_json::Value = match serde_json::from_str(line) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("mcp: ignoring malformed JSON-RPC line: {e}");
                continue;
            }
        };
        if let Some(resp) = handle_mcp_message(&msg, &ctx, &token).await {
            let mut out = serde_json::to_string(&resp).unwrap_or_default();
            out.push('\n');
            stdout.write_all(out.as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}
