use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use elastos_runtime::provider::ProviderRegistry;
use tokio::net::TcpListener;

use super::{gateway_router_with_api_url, GatewayState, GATEWAY_VERSION};

pub async fn start_gateway_server(
    addr: &str,
    provider_registry: Option<Arc<ProviderRegistry>>,
    cache_dir: PathBuf,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    crate::auth::verify_auth_audit_chain_ready(&data_dir)?;
    let listener = TcpListener::bind(addr).await?;
    let gateway_api_url = trusted_gateway_api_url(addr)?;
    let state = GatewayState {
        provider_registry,
        identity_manager: Arc::new(OnceLock::new()),
        cache_dir,
        data_dir,
    };
    let app = gateway_router_with_api_url(state, gateway_api_url);
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
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        shutdown_signal().await;
        println!("\nShutting down gateway...");
    })
    .await?;
    Ok(())
}

fn trusted_gateway_api_url(addr: &str) -> anyhow::Result<String> {
    let authority = addr
        .parse::<axum::http::uri::Authority>()
        .map_err(|err| anyhow::anyhow!("invalid Gateway bind address {addr}: {err}"))?;
    let port = authority
        .port_u16()
        .ok_or_else(|| anyhow::anyhow!("Gateway bind address is missing a port"))?;
    let host = match authority.host().parse::<IpAddr>() {
        Ok(ip) if ip.is_unspecified() => "localhost".to_string(),
        _ => authority.host().to_string(),
    };
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    Ok(format!("http://{authority}"))
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

#[cfg(test)]
mod trusted_gateway_tests {
    use super::*;

    #[test]
    fn trusted_gateway_api_url_preserves_operator_localhost() {
        assert_eq!(
            trusted_gateway_api_url("localhost:61180").unwrap(),
            "http://localhost:61180"
        );
    }

    #[test]
    fn trusted_gateway_api_url_replaces_unspecified_bind_address() {
        assert_eq!(
            trusted_gateway_api_url("0.0.0.0:8090").unwrap(),
            "http://localhost:8090"
        );
    }
}
