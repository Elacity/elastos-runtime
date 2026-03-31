use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use elastos_runtime::provider;

use crate::{
    api,
    binaries::{find_installed_provider_binary, verify_component_binary_with_data_dir},
    setup,
    sources::default_data_dir,
    supervisor,
};

const ELASTOS_VERSION: &str = env!("ELASTOS_VERSION");

pub struct GatewayControlPlane {
    pub provider_registry: Arc<provider::ProviderRegistry>,
}

pub async fn run_gateway_direct<F, Fut>(
    addr: String,
    public: bool,
    cache_dir: Option<PathBuf>,
    publish: Option<PathBuf>,
    setup_control_plane: F,
) -> anyhow::Result<()>
where
    F: Fn() -> Fut + Copy + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<GatewayControlPlane>> + Send,
{
    if public {
        return run_gateway_public(addr, cache_dir, publish, setup_control_plane).await;
    }

    eprintln!("[gateway] ElastOS {} starting on {}", ELASTOS_VERSION, addr);

    let data_dir = default_data_dir();
    let ipfs_binary = find_installed_provider_binary("ipfs-provider");
    if ipfs_binary.is_none() {
        eprintln!("[gateway] Warning: ipfs-provider not found. IPFS fetch will fail.");
        eprintln!("[gateway] Install with: elastos setup --with ipfs-provider");
    }

    let cache_path = cache_dir.unwrap_or_else(|| data_dir.join("gateway-cache"));
    std::fs::create_dir_all(&cache_path)?;

    api::gateway::start_gateway_server(&addr, ipfs_binary, None, cache_path, data_dir).await
}

async fn run_gateway_public<F, Fut>(
    addr: String,
    cache_dir: Option<PathBuf>,
    publish: Option<PathBuf>,
    setup_control_plane: F,
) -> anyhow::Result<()>
where
    F: Fn() -> Fut + Copy + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<GatewayControlPlane>> + Send,
{
    eprintln!(
        "[gateway] ElastOS {} starting public gateway on {}",
        ELASTOS_VERSION, addr
    );

    let data_dir = default_data_dir();
    let components_path = data_dir.join("components.json");
    if !components_path.exists() {
        anyhow::bail!(
            "components.json not found at {}. Run: elastos setup --list",
            components_path.display()
        );
    }

    let bind_addr = addr.replace("127.0.0.1", "0.0.0.0");

    let components_data = tokio::fs::read_to_string(&components_path).await?;
    let registry: setup::ComponentsManifest = serde_json::from_str(&components_data)?;

    let control_plane = setup_control_plane().await?;
    for external in ["ipfs-provider", "kubo"] {
        if registry.external.contains_key(external) {
            eprintln!("[gateway] ensuring external '{}'", external);
            supervisor_require_ok(
                &supervisor::Supervisor::new(data_dir.clone(), registry.clone()),
                supervisor::SupervisorRequest::DownloadExternal {
                    name: external.to_string(),
                    platform: setup::detect_platform(),
                },
            )
            .await?;
        }
    }
    register_installed_ipfs_provider(&data_dir, &control_plane.provider_registry).await?;

    let mut sup_inner = supervisor::Supervisor::new(data_dir.clone(), registry);
    sup_inner.set_provider_registry(control_plane.provider_registry.clone());
    // Infrastructure trust domain: gateway capsules (ipfs-provider, tunnel-provider)
    // are trusted service-plane components, not user application capsules.  They run
    // under runtime/service authority and are explicitly outside the user shell
    // approval model.  No CapabilityManager or PendingRequestStore is attached —
    // if an infrastructure capsule ever requests a capability, the bridge returns
    // a clear "infrastructure_capsule" denial rather than silently granting.
    let sup = Arc::new(sup_inner);

    eprintln!(
        "[gateway] Trust domain: infrastructure (service-plane capsules, no user shell approval)"
    );

    let (tunnel_capsules, tunnel_externals) = sup.resolve_launch_plan("tunnel-provider").await?;

    let mut externals = Vec::<String>::new();
    for ext in tunnel_externals {
        if !externals.contains(&ext) {
            externals.push(ext);
        }
    }

    let mut capsules = Vec::<String>::new();
    for cap in tunnel_capsules {
        if !capsules.contains(&cap) {
            capsules.push(cap);
        }
    }

    for ext in &externals {
        eprintln!("[gateway] ensuring external '{}'", ext);
        supervisor_require_ok(
            &sup,
            supervisor::SupervisorRequest::DownloadExternal {
                name: ext.clone(),
                platform: setup::detect_platform(),
            },
        )
        .await?;
    }

    for cap in &capsules {
        eprintln!("[gateway] ensuring capsule '{}'", cap);
        supervisor_require_ok(
            &sup,
            supervisor::SupervisorRequest::EnsureCapsule { name: cap.clone() },
        )
        .await?;
    }

    let mut handles = Vec::<String>::new();
    let mut tunnel_host_ip: Option<String> = None;
    for cap in &capsules {
        eprintln!("[gateway] launching infrastructure capsule '{}'", cap);
        let resp = supervisor_require_ok(
            &sup,
            supervisor::SupervisorRequest::LaunchCapsule {
                name: cap.clone(),
                config: serde_json::json!({}),
            },
        )
        .await?;
        let handle = resp
            .handle
            .ok_or_else(|| anyhow::anyhow!("launch response missing handle for '{}'", cap))?;
        if cap == "tunnel-provider" {
            tunnel_host_ip = resp.path.clone();
        }
        handles.push(handle);
    }

    let gateway_resp = supervisor_require_ok(
        &sup,
        supervisor::SupervisorRequest::StartGateway {
            addr: bind_addr.clone(),
            cache_dir: cache_dir.map(|p| p.to_string_lossy().to_string()),
        },
    )
    .await?;
    let effective_addr = gateway_resp.path.unwrap_or(bind_addr);

    println!("Gateway: http://{}", effective_addr);

    let port = effective_addr.rsplit(':').next().unwrap_or("8090");
    let tunnel_target = if let Some(ref host_ip) = tunnel_host_ip {
        format!("http://{}:{}", host_ip, port)
    } else {
        format!("http://{}", effective_addr)
    };

    eprintln!("[gateway] requesting public tunnel for {}", tunnel_target);
    let tunnel_resp = provider_send_raw_retry(
        &control_plane.provider_registry,
        "tunnel",
        &serde_json::json!({
            "op": "start",
            "target": tunnel_target,
        }),
        std::time::Duration::from_secs(30),
    )
    .await?;
    eprintln!("[gateway] tunnel-provider start: {}", tunnel_resp);
    let url = wait_for_public_tunnel_url(
        &control_plane.provider_registry,
        std::time::Duration::from_secs(90),
    )
    .await?;
    println!("Public URL: {}", url);

    if let Some(ref publish_path) = publish {
        match publish_to_ipfs(&control_plane.provider_registry, publish_path).await {
            Ok(cid) => {
                let filename = publish_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".to_string());
                let install_url = format!("{}/s/{}/{}", url, cid, filename);

                println!();
                println!("Install:   curl -fsSL {} | bash", install_url);
            }
            Err(e) => {
                eprintln!("[gateway] failed to publish {:?}: {}", publish_path, e);
                println!("Installer URL template: {}/s/<cid>/install.sh", url);
            }
        }
    }

    eprintln!("[gateway] runtime gateway running; press Ctrl+C to stop");
    tokio::signal::ctrl_c().await?;

    for handle in handles.iter().rev() {
        let _ = sup
            .handle_request(supervisor::SupervisorRequest::StopCapsule {
                handle: handle.clone(),
            })
            .await;
    }

    Ok(())
}

async fn register_installed_ipfs_provider(
    data_dir: &Path,
    registry: &Arc<provider::ProviderRegistry>,
) -> anyhow::Result<()> {
    let ipfs_binary = find_installed_provider_binary("ipfs-provider").ok_or_else(|| {
        anyhow::anyhow!(
            "ipfs-provider not found. Install with:\n\n  elastos setup --with kubo --with ipfs-provider"
        )
    })?;
    verify_component_binary_with_data_dir(data_dir, "ipfs-provider", &ipfs_binary)?;

    let bridge = provider::ProviderBridge::spawn(&ipfs_binary, Default::default())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to spawn ipfs-provider: {}", e))?;
    let ipfs_provider: Arc<dyn provider::Provider> = Arc::new(
        provider::CapsuleProvider::with_scheme(Arc::new(bridge), "ipfs"),
    );
    registry
        .register_sub_provider("ipfs", ipfs_provider)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to register elastos://ipfs sub-provider: {}", e))
}

async fn supervisor_require_ok(
    sup: &supervisor::Supervisor,
    req: supervisor::SupervisorRequest,
) -> anyhow::Result<supervisor::SupervisorResponse> {
    let resp = sup.handle_request(req).await;
    if resp.status != "ok" {
        anyhow::bail!(
            "{}",
            resp.error
                .unwrap_or_else(|| "supervisor request failed".to_string())
        );
    }
    Ok(resp)
}

async fn provider_send_raw_retry(
    registry: &Arc<provider::ProviderRegistry>,
    scheme: &str,
    request: &serde_json::Value,
    timeout: std::time::Duration,
) -> anyhow::Result<serde_json::Value> {
    let started = std::time::Instant::now();
    let last_err = loop {
        match registry.send_raw(scheme, request).await {
            Ok(value) => return Ok(value),
            Err(err) => {
                let message = err.to_string();
                if started.elapsed() >= timeout {
                    break message;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    };
    anyhow::bail!(
        "provider '{}' did not become ready within {:?}: {}",
        scheme,
        timeout,
        last_err
    )
}

async fn wait_for_public_tunnel_url(
    registry: &Arc<provider::ProviderRegistry>,
    timeout: std::time::Duration,
) -> anyhow::Result<String> {
    let started = std::time::Instant::now();
    let mut last_log_seen: Option<String> = None;

    loop {
        let status = provider_send_raw_retry(
            registry,
            "tunnel",
            &serde_json::json!({ "op": "status" }),
            std::time::Duration::from_secs(5),
        )
        .await?;

        let data = status
            .get("data")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let running = data
            .get("running")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let url = data.get("url").and_then(|u| u.as_str());
        let last_log = data
            .get("last_log")
            .and_then(|l| l.as_str())
            .map(ToOwned::to_owned);

        if let Some(url) = url {
            return Ok(url.to_string());
        }

        if let Some(log) = last_log.clone() {
            if last_log_seen.as_deref() != Some(log.as_str()) {
                eprintln!("[gateway] tunnel: {}", log);
                last_log_seen = Some(log);
            }
        }

        if !running {
            anyhow::bail!(
                "tunnel-provider exited before publishing a public URL{}",
                last_log_seen
                    .as_deref()
                    .map(|log| format!(": {}", log))
                    .unwrap_or_default()
            );
        }

        if started.elapsed() >= timeout {
            anyhow::bail!(
                "timed out waiting for public URL{}",
                last_log_seen
                    .as_deref()
                    .map(|log| format!(" (last log: {})", log))
                    .unwrap_or_default()
            );
        }

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

async fn publish_to_ipfs(
    registry: &Arc<provider::ProviderRegistry>,
    path: &Path,
) -> anyhow::Result<String> {
    let content = tokio::fs::read(path).await?;
    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let data_b64 = base64::engine::general_purpose::STANDARD.encode(&content);
    eprintln!(
        "[gateway] publishing {} ({} bytes) to IPFS...",
        filename,
        content.len()
    );
    let resp = provider_send_raw_retry(
        registry,
        "ipfs",
        &serde_json::json!({
            "op": "add_directory",
            "files": [{ "path": filename, "data": data_b64 }],
            "pin": true,
        }),
        std::time::Duration::from_secs(30),
    )
    .await?;
    let cid = resp
        .get("data")
        .and_then(|d| d.get("cid"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| anyhow::anyhow!("no CID in ipfs-provider response: {}", resp))?;
    eprintln!("[gateway] published {} as CID {}", filename, cid);
    Ok(cid.to_string())
}
