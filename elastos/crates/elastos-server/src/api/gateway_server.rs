use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use elastos_runtime::provider::ProviderRegistry;
use tokio::net::TcpListener;

use super::{gateway_router, GatewayState, GATEWAY_VERSION};

#[allow(clippy::too_many_arguments)]
pub async fn start_gateway_server(
    addr: &str,
    provider_registry: Option<Arc<ProviderRegistry>>,
    cache_dir: PathBuf,
    data_dir: PathBuf,
    spend_policy: Option<crate::carrier_bridge::SpendPolicy>,
    shared_audit_log: Option<Arc<elastos_runtime::primitives::audit::AuditLog>>,
    standing_service: Option<Arc<elastos_runtime::capability::intent::StandingGrantService>>,
    capability_manager: Option<Arc<elastos_runtime::capability::CapabilityManager>>,
) -> anyhow::Result<()> {
    let data_dir_for_mandates = data_dir.clone();
    let state = GatewayState {
        provider_registry,
        identity_manager: Arc::new(OnceLock::new()),
        cache_dir,
        data_dir,
        // Unify onto the shared runtime custody chain when it is durable; otherwise the
        // gateway opens its own durable file sink (never a durable→memory downgrade).
        audit_log: super::seed_gateway_audit_log(shared_audit_log),
        spend_policy,
    };
    let mut app = gateway_router(state);
    // Merge the mandates sub-router (reads + the revoke kill switch) only when BOTH the shared
    // registry and the capability manager are present — the manager owns the durable audit chain
    // the receipt export, the token-revocation liveness check, AND the attested revoke all need.
    // Without it the app would be crippled, so we fail closed to "no live mandate data" rather
    // than half-wire it. The sub-router carries its own state, so the ~40 GatewayState
    // construction sites are left untouched.
    if let (Some(standing_service), Some(capability_manager)) =
        (standing_service, capability_manager)
    {
        app = app.merge(super::gateway_mandates::mandate_router(
            super::gateway_mandates::MandateApiState {
                standing_service,
                capability_manager,
                data_dir: data_dir_for_mandates,
            },
        ));
    }
    let listener = TcpListener::bind(addr).await?;
    let advertised = advertised_gateway_urls(addr);
    println!("ElastOS Gateway v{}", GATEWAY_VERSION);
    println!("  Bind:      http://{}", addr);
    if let Some(primary) = advertised.first() {
        println!("  Open:      {}", primary);
        println!("  Room:      {}apps/chat-room/", primary);
        println!("  Content:   {}s/<cid>/", primary);
        for extra in advertised.iter().skip(1) {
            println!("  Also:      {}", extra);
        }
    } else {
        println!("  Open:      http://{}", addr);
        println!("  Room:      http://{}/apps/chat-room/", addr);
        println!("  Content:   http://{}/s/<cid>/", addr);
    }
    println!();
    println!("  Cache is unbounded (Tier 1) — delete cache dir to reclaim space");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            shutdown_signal().await;
            println!("\nShutting down gateway...");
        })
        .await?;
    Ok(())
}

pub(crate) fn advertised_gateway_urls(addr: &str) -> Vec<String> {
    let Ok(socket_addr) = addr.parse::<SocketAddr>() else {
        return vec![format!("http://{}/", addr.trim_end_matches('/'))];
    };

    let port = socket_addr.port();
    let host = socket_addr.ip();

    let mut urls = Vec::new();
    match host {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            urls.push(format!("http://127.0.0.1:{}/", port));
            for ip in detect_advertisable_ips() {
                if ip.is_loopback() {
                    continue;
                }
                urls.push(format!("http://{}:{}/", ip, port));
            }
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            urls.push(format!("http://[::1]:{}/", port));
            for ip in detect_advertisable_ips() {
                if ip.is_loopback() {
                    continue;
                }
                urls.push(match ip {
                    IpAddr::V4(ip) => format!("http://{}:{}/", ip, port),
                    IpAddr::V6(ip) => format!("http://[{}]:{}/", ip, port),
                });
            }
        }
        IpAddr::V4(ip) => {
            urls.push(format!("http://{}:{}/", ip, port));
        }
        IpAddr::V6(ip) => {
            urls.push(format!("http://[{}]:{}/", ip, port));
        }
    }

    dedupe_urls(urls)
}

fn detect_advertisable_ips() -> Vec<IpAddr> {
    let mut ips = Vec::new();
    if let Ok(output) = std::process::Command::new("hostname").arg("-I").output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for part in stdout.split_whitespace() {
                if let Ok(ip) = part.parse::<IpAddr>() {
                    ips.push(ip);
                }
            }
        }
    }
    if ips.is_empty() {
        ips.push("127.0.0.1".parse().unwrap());
    }
    ips
}

fn dedupe_urls(urls: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for url in urls {
        if seen.insert(url.clone()) {
            deduped.push(url);
        }
    }
    deduped
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = ctrl_c => {},
                _ = terminate.recv() => {},
            }
        } else {
            ctrl_c.await;
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await;
    }
}
