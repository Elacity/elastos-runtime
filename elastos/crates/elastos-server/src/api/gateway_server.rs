use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use elastos_runtime::provider::ProviderRegistry;
use tokio::net::TcpListener;

use super::{gateway_router_with_api_url, GatewayState, GATEWAY_VERSION};

#[derive(Clone, Default)]
pub struct GatewayCollaborationContext {
    pub chat_product_port: Option<crate::collaboration_product::CollaborationChatProductPort>,
    pub presence_product_port:
        Option<crate::collaboration_presence::CollaborationPresenceProductPort>,
    pub discovery_service:
        Option<crate::collaboration_discovery_runtime::CollaborationDiscoveryService>,
}

pub async fn start_gateway_server(
    addr: &str,
    provider_registry: Option<Arc<ProviderRegistry>>,
    collaboration_chat_product_port: Option<
        crate::collaboration_product::CollaborationChatProductPort,
    >,
    collaboration_presence_product_port: Option<
        crate::collaboration_presence::CollaborationPresenceProductPort,
    >,
    cache_dir: PathBuf,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    start_gateway_server_with_collaboration_context(
        addr,
        provider_registry,
        GatewayCollaborationContext {
            chat_product_port: collaboration_chat_product_port,
            presence_product_port: collaboration_presence_product_port,
            discovery_service: None,
        },
        cache_dir,
        data_dir,
    )
    .await
}

pub(crate) async fn start_gateway_server_with_collaboration_context(
    addr: &str,
    provider_registry: Option<Arc<ProviderRegistry>>,
    collaboration: GatewayCollaborationContext,
    cache_dir: PathBuf,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    start_gateway_server_with_ready(
        addr,
        provider_registry,
        collaboration,
        cache_dir,
        data_dir,
        None,
    )
    .await
}

pub(crate) async fn start_gateway_server_with_ready(
    addr: &str,
    provider_registry: Option<Arc<ProviderRegistry>>,
    collaboration: GatewayCollaborationContext,
    cache_dir: PathBuf,
    data_dir: PathBuf,
    on_ready: Option<fn(&str)>,
) -> anyhow::Result<()> {
    start_gateway_server_with_shutdown(
        addr,
        provider_registry,
        collaboration,
        cache_dir,
        data_dir,
        on_ready,
        shutdown_signal(),
    )
    .await
}

async fn start_gateway_server_with_shutdown(
    addr: &str,
    provider_registry: Option<Arc<ProviderRegistry>>,
    collaboration: GatewayCollaborationContext,
    cache_dir: PathBuf,
    data_dir: PathBuf,
    on_ready: Option<fn(&str)>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    crate::auth::verify_auth_audit_chain_ready(&data_dir)?;
    let listener = TcpListener::bind(addr).await?;
    let gateway_local_control = match provider_registry.as_ref() {
        Some(registry) => Some(
            crate::api::gateway_local_control::start_gateway_local_control(
                &data_dir,
                registry.clone(),
            )
            .await?,
        ),
        None => None,
    };
    let gateway_api_url = trusted_gateway_api_url(addr)?;
    let state = GatewayState {
        provider_registry,
        collaboration_chat_product_port: collaboration.chat_product_port,
        collaboration_presence_product_port: collaboration.presence_product_port,
        collaboration_discovery_service: collaboration.discovery_service,
        identity_manager: Arc::new(OnceLock::new()),
        cache_dir,
        data_dir,
    };
    let browser_lifecycle_reconciler =
        super::gateway_browser::start_browser_lifecycle_reconciler(state.clone())
            .map_err(anyhow::Error::msg)?;
    let home_url = format!("{gateway_api_url}/home/");
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
    if let Some(on_ready) = on_ready {
        on_ready(&home_url);
    }
    let connections = axum_server::Handle::new();
    let server = axum_server::Server::<SocketAddr>::from_listener(listener)
        .handle(connections.clone())
        .serve(app.into_make_service_with_connect_info::<SocketAddr>());
    tokio::pin!(server);
    let serve_result = tokio::select! {
        result = &mut server => result,
        _ = shutdown => {
            println!("\nShutting down gateway...");
            // Bound stream draining, then cancel the owned HTTP connections.
            connections.graceful_shutdown(Some(std::time::Duration::from_secs(2)));
            (&mut server).await
        }
    };
    connections.shutdown();
    // The server's deadline signals connection tasks. Wait for their request
    // futures to drop before cleanup can release this data root to a new host.
    while connections.connection_count() != 0 {
        tokio::task::yield_now().await;
    }
    browser_lifecycle_reconciler.cancel();
    let reconciliation_result = browser_lifecycle_reconciler.join().await;
    let local_control_result = async {
        if let Some(control) = gateway_local_control {
            control.shutdown().await?;
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    serve_result?;
    reconciliation_result.map_err(anyhow::Error::msg)?;
    local_control_result?;
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
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::oneshot;

    static READY_URL: Mutex<Option<oneshot::Sender<String>>> = Mutex::new(None);

    fn record_ready(url: &str) {
        let parsed = url::Url::parse(url).unwrap();
        // The callback must run after a listener exists, even before HTTP is polled.
        std::net::TcpStream::connect((parsed.host_str().unwrap(), parsed.port().unwrap())).unwrap();
        READY_URL
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(url.to_string())
            .unwrap();
    }

    fn unexpected_ready(_: &str) {
        panic!("failed gateway startup must not open Home");
    }

    fn unused_localhost_address() -> String {
        let listener = std::net::TcpListener::bind("localhost:0").unwrap();
        format!("localhost:{}", listener.local_addr().unwrap().port())
    }

    #[tokio::test]
    async fn browser_home_ready_serves_router_and_shutdown_releases_resources() {
        let temp = tempfile::tempdir().unwrap();
        let addr = unused_localhost_address();
        let (ready_tx, ready_rx) = oneshot::channel();
        *READY_URL.lock().unwrap() = Some(ready_tx);
        let (stop_tx, stop_rx) = oneshot::channel();
        let task_addr = addr.clone();
        let data = temp.path().to_path_buf();
        let task_data = data.clone();
        // Match the real gateway caller's owned child guard. Even an open HTTP
        // body must let gateway shutdown return so this guard can reap its child.
        let helper = crate::api::server::HostHelperProcess {
            name: "gateway shutdown test",
            child: std::process::Command::new("sleep")
                .arg("60")
                .spawn()
                .unwrap(),
        };
        let helper_pid = helper.child.id();
        let server = tokio::spawn(async move {
            let _helper = helper;
            start_gateway_server_with_shutdown(
                &task_addr,
                Some(Arc::new(ProviderRegistry::new())),
                GatewayCollaborationContext::default(),
                task_data.join("cache"),
                task_data,
                Some(record_ready),
                async {
                    let _ = stop_rx.await;
                },
            )
            .await
        });
        let home = tokio::time::timeout(Duration::from_secs(5), ready_rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(home, format!("http://{addr}/home/"));
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{addr}/healthz"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);

        // A late startup failure after binding must still leave the callback untouched.
        let duplicate = start_gateway_server_with_ready(
            &unused_localhost_address(),
            None,
            GatewayCollaborationContext::default(),
            data.join("cache"),
            data.clone(),
            Some(unexpected_ready),
        )
        .await
        .unwrap_err();
        assert!(duplicate
            .to_string()
            .contains("already running for this data root"));

        // Expect:100 proves Axum started reading this streaming request body.
        // Keep it incomplete while shutdown begins, as an SSE/WebSocket client
        // likewise keeps an in-flight connection alive after the listener closes.
        let mut streaming = tokio::net::TcpStream::connect(&addr).await.unwrap();
        streaming
            .write_all(format!(
                "POST /api/auth/passkey/register/begin HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: 1000\r\nExpect: 100-continue\r\n\r\n"
            ).as_bytes())
            .await
            .unwrap();
        let mut interim = [0; 128];
        let length = tokio::time::timeout(Duration::from_secs(2), streaming.read(&mut interim))
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&interim[..length]).starts_with("HTTP/1.1 100 Continue"));
        streaming.write_all(b"{").await.unwrap();
        let coords_path = data.join("gateway-runtime-coords.json");
        assert!(coords_path.is_file());

        stop_tx.send(()).unwrap();
        let mut server = server;
        let stopped = tokio::time::timeout(Duration::from_secs(5), &mut server).await;
        if stopped.is_err() {
            server.abort();
            let _ = server.await;
            panic!("gateway shutdown waited for a client stream to close");
        }
        stopped.unwrap().unwrap().unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(1), streaming.read(&mut interim))
            .await
            .expect("shutdown must close the held client connection");
        assert!(matches!(closed, Ok(0)) || closed.is_err());
        // Completing the body after shutdown cannot resume its request handler.
        let remainder = format!("}}{}", " ".repeat(998));
        let _ = streaming.write_all(remainder.as_bytes()).await;
        let after_write =
            tokio::time::timeout(Duration::from_secs(1), streaming.read(&mut interim))
                .await
                .expect("the closed connection must remain closed");
        assert!(matches!(after_write, Ok(0)) || after_write.is_err());
        assert!(
            !coords_path.exists(),
            "local control ownership must be released"
        );
        #[cfg(unix)]
        {
            assert_eq!(unsafe { libc::kill(helper_pid as i32, 0) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
        // Reusing both the address and data root proves listener/reconciler cleanup.
        start_gateway_server_with_shutdown(
            &addr,
            None,
            GatewayCollaborationContext::default(),
            data.join("cache"),
            data,
            None,
            async {},
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn browser_home_ready_is_skipped_when_port_is_occupied() {
        let temp = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let error = start_gateway_server_with_ready(
            &listener.local_addr().unwrap().to_string(),
            None,
            GatewayCollaborationContext::default(),
            temp.path().join("cache"),
            temp.path().to_path_buf(),
            Some(unexpected_ready),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::AddrInUse
        );
    }

    #[tokio::test]
    async fn browser_home_ready_is_skipped_when_auth_setup_fails() {
        let temp = tempfile::tempdir().unwrap();
        let path = crate::auth::auth_state_path(temp.path()).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"invalid auth state").unwrap();
        let error = start_gateway_server_with_ready(
            &unused_localhost_address(),
            None,
            GatewayCollaborationContext::default(),
            temp.path().join("cache"),
            temp.path().to_path_buf(),
            Some(unexpected_ready),
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("failed to parse auth state"),
            "{error}"
        );
    }

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
