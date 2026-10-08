use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use elastos_runtime::provider::ProviderRegistry;
use tokio::net::TcpListener;

use super::{gateway_router_with_api_url, GatewayState, GATEWAY_VERSION};

#[derive(Clone, Default)]
pub struct GatewayCollaborationContext {
    pub carrier_endpoint: Option<iroh::Endpoint>,
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
            carrier_endpoint: None,
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
        shutdown_signal()?,
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
    let local_control_registry = provider_registry.clone();
    let gateway_api_url = if addr.parse::<axum::http::uri::Authority>()?.port_u16() == Some(0) {
        trusted_gateway_api_url(&listener.local_addr()?.to_string())?
    } else {
        trusted_gateway_api_url(addr)?
    };
    super::gateway_frontdoor::GatewayFrontDoor::load(&data_dir, &gateway_api_url)?;
    let state = GatewayState {
        carrier_endpoint: collaboration.carrier_endpoint,
        provider_registry,
        collaboration_chat_product_port: collaboration.chat_product_port,
        collaboration_presence_product_port: collaboration.presence_product_port,
        collaboration_discovery_service: collaboration.discovery_service,
        identity_manager: Arc::new(OnceLock::new()),
        cache_dir,
        data_dir: data_dir.clone(),
    };
    let browser_lifecycle_reconciler =
        super::gateway_browser::start_browser_lifecycle_reconciler(state.clone())
            .map_err(anyhow::Error::msg)?;
    let home_url = format!("{gateway_api_url}/home/");
    let app = gateway_router_with_api_url(state, gateway_api_url);
    // Publish one complete coordinate identity after Home setup, before children
    // capture its owner generation. A later mutation would change their owner hash.
    let gateway_local_control = match local_control_registry {
        Some(registry) => {
            match crate::api::gateway_local_control::start_gateway_local_control_with_home_url(
                &data_dir,
                registry,
                home_url.clone(),
            )
            .await
            {
                Ok(control) => Some(control),
                Err(error) => {
                    browser_lifecycle_reconciler.cancel();
                    let _ = browser_lifecycle_reconciler.join().await;
                    return Err(error);
                }
            }
        }
        None => None,
    };
    let managed_owner = crate::runtime_control::gateway_children::Owner::read(&data_dir).ok();
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
    // Finish any synchronous spawn/registration before retiring this owner or
    // closing a Terminal which may be the launcher in that critical section.
    let ownership_lock = managed_owner
        .as_ref()
        .map(|owner| owner.lock())
        .transpose()?;
    let local_control_result = async {
        if let Some(control) = gateway_local_control {
            control.shutdown().await?;
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    super::gateway_home_terminal::shutdown_home_terminal_sessions().await;
    let managed_shutdown_result =
        crate::runtime_control::gateway_children::shutdown(managed_owner).await;
    drop(ownership_lock);
    serve_result?;
    reconciliation_result.map_err(anyhow::Error::msg)?;
    local_control_result?;
    managed_shutdown_result?;
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

fn shutdown_signal() -> anyhow::Result<impl std::future::Future<Output = ()> + Send + 'static> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        // Register synchronously so TERM during startup remains queued for drain.
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        Ok(async move {
            tokio::select! {
                _ = interrupt.recv() => {},
                _ = terminate.recv() => {},
            }
        })
    }
    #[cfg(not(unix))]
    {
        Ok(async {
            let _ = tokio::signal::ctrl_c().await;
        })
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

    #[cfg(unix)]
    #[tokio::test]
    async fn term_drains_gateway_streams_and_separate_owned_child_groups() {
        use crate::update_controller::child::OwnedChild;
        use std::process::Stdio;

        let temp = tempfile::tempdir().unwrap();
        let fixture = format!(
            "{}::gateway_term_fixture",
            module_path!().split_once("::").unwrap().1
        );
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", &fixture, "--ignored", "--nocapture"])
            .env("ELASTOS_GATEWAY_TERM_FIXTURE_ROOT", temp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = OwnedChild::spawn(&mut command).unwrap();
        let pid = child.pid();
        let root = temp.path().to_path_buf();
        let journey = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            let pids_path = root.join("owned-pids.json");
            while !pids_path.exists() {
                assert!(
                    crate::update_controller::child::observe_exit(pid)
                        .unwrap()
                        .is_none(),
                    "gateway fixture exited before readiness"
                );
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "gateway fixture did not become ready"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let pids: Vec<u32> =
                serde_json::from_slice(&std::fs::read(pids_path).unwrap()).unwrap();
            let home = std::fs::read_to_string(root.join("home-url")).unwrap();
            let coords_path = crate::runtime_control::gateway_runtime_coord_path(&root);
            let coords: crate::runtime_control::RuntimeCoords =
                serde_json::from_slice(&std::fs::read(&coords_path).unwrap()).unwrap();
            assert_eq!(coords.pid, pid);
            assert_eq!(coords.home_url, home);
            let home = url::Url::parse(&home).unwrap();
            let public_authority = home.authority();
            let control_authority = coords.api_url.strip_prefix("http://").unwrap();
            let public = tokio::net::TcpStream::connect(public_authority)
                .await
                .unwrap();
            let control = tokio::net::TcpStream::connect(control_authority)
                .await
                .unwrap();
            let mut streams = [public, control];
            let requests = [
                format!("POST /api/auth/passkey/register/begin HTTP/1.1\r\nHost: {public_authority}\r\nOrigin: {}\r\nContent-Type: application/json\r\nContent-Length: 1000\r\nExpect: 100-continue\r\n\r\n", home.origin().ascii_serialization()),
                format!("POST /api/auth/attach HTTP/1.1\r\nHost: {control_authority}\r\nContent-Type: application/json\r\nContent-Length: 1000\r\nExpect: 100-continue\r\n\r\n"),
            ];
            let mut buffer = [0; 128];
            for (stream, request) in streams.iter_mut().zip(requests) {
                stream.write_all(request.as_bytes()).await.unwrap();
                let count = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buffer))
                    .await
                    .unwrap()
                    .unwrap();
                assert!(
                    String::from_utf8_lossy(&buffer[..count]).starts_with("HTTP/1.1 100 Continue")
                );
                stream.write_all(b"{").await.unwrap();
            }
            assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) }, 0);
            let status = loop {
                if let Some(status) = crate::update_controller::child::observe_exit(pid).unwrap() {
                    break status;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "TERM did not finish gateway drain"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            assert!(status.success(), "gateway TERM fixture failed: {status}");
            assert!(!coords_path.exists());
            for pid in pids {
                assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, -1);
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::ESRCH)
                );
            }
            for stream in &mut streams {
                let closed =
                    tokio::time::timeout(Duration::from_millis(200), stream.read(&mut buffer))
                        .await
                        .unwrap();
                assert!(matches!(closed, Ok(0)) || closed.is_err());
            }
        });
        let result = journey.await;
        // Keep the child and test Home alive through cleanup, including assertion panic.
        child.stop().await.unwrap();
        result.unwrap();
    }

    #[cfg(unix)]
    fn gateway_term_ready(url: &str) {
        let root = PathBuf::from(std::env::var_os("ELASTOS_GATEWAY_TERM_FIXTURE_ROOT").unwrap());
        std::fs::write(root.join("home-url"), url).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "isolated subprocess fixture for term_drains_gateway_streams_and_separate_owned_child_groups"]
    async fn gateway_term_fixture() {
        let root = PathBuf::from(std::env::var_os("ELASTOS_GATEWAY_TERM_FIXTURE_ROOT").unwrap());
        let shutdown = shutdown_signal().unwrap();
        crate::update_controller::watch_parent().unwrap();
        let helper = crate::api::server::HostHelperProcess {
            name: "TERM drain fixture helper",
            child: std::process::Command::new("sleep")
                .arg("60")
                .spawn()
                .unwrap(),
        };
        let helper_pid = helper.child.id();
        let child_root = root.clone();
        let managed = tokio::spawn(async move {
            let coords_path = crate::runtime_control::gateway_runtime_coord_path(&child_root);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
            while !coords_path.exists() {
                assert!(tokio::time::Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let owner = crate::runtime_control::gateway_children::Owner::read(&child_root).unwrap();
            let (mut child, descendant_pid) = crate::runtime_control::gateway_children::test_child(
                &owner,
                &child_root.join("managed-runtime.json"),
            )
            .await;
            let pid = child.child.id();
            child.ready();
            drop(child);
            let pids_pending = child_root.join("owned-pids.pending");
            std::fs::write(
                &pids_pending,
                serde_json::to_vec(&[helper_pid, pid, descendant_pid]).unwrap(),
            )
            .unwrap();
            // The parent treats this path as readiness; publish only complete JSON.
            std::fs::rename(pids_pending, child_root.join("owned-pids.json")).unwrap();
        });
        start_gateway_server_with_shutdown(
            "127.0.0.1:0",
            Some(Arc::new(ProviderRegistry::new())),
            GatewayCollaborationContext::default(),
            root.join("cache"),
            root,
            Some(gateway_term_ready),
            shutdown,
        )
        .await
        .unwrap();
        managed.await.unwrap();
        drop(helper);
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
        let journey_data = data.clone();
        let journey_addr = addr.clone();
        // A joined task returns assertion failures so gateway cleanup still runs.
        let journey = tokio::spawn(async move {
            let data = journey_data;
            let addr = journey_addr;
            let home = tokio::time::timeout(Duration::from_secs(5), ready_rx)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(home, format!("http://{addr}/home/"));
            let home_url = url::Url::parse(&home).unwrap();
            let authority = home_url.authority();
            let origin = home_url.origin().ascii_serialization();
            let owner = crate::runtime_control::gateway_children::Owner::read(&data).unwrap();
            let managed_coords_path = data.join("runtime-coords-home.json");
            let (mut managed, managed_helper_pid) =
                crate::runtime_control::gateway_children::test_child(&owner, &managed_coords_path)
                    .await;
            let managed_pid = managed.child.id();
            managed.ready();
            drop(managed);
            let unrelated = crate::api::server::HostHelperProcess {
                name: "unrelated runtime fixture",
                child: std::process::Command::new("sleep")
                    .arg("60")
                    .spawn()
                    .unwrap(),
            };
            let response = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap()
                .get(home_url.join("/healthz").unwrap())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::OK);

            // A late startup failure after binding must still leave the callback untouched.
            let owned_coords_path = crate::runtime_control::gateway_runtime_coord_path(&data);
            let owned_coords = std::fs::read(&owned_coords_path).unwrap();
            let duplicate = start_gateway_server_with_ready(
                &unused_localhost_address(),
                Some(Arc::new(ProviderRegistry::new())),
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
            assert_eq!(std::fs::read(&owned_coords_path).unwrap(), owned_coords);

            // Expect:100 proves Axum started reading this streaming request body.
            // Keep it incomplete while shutdown begins, as an SSE/WebSocket client
            // likewise keeps an in-flight connection alive after the listener closes.
            let mut streaming = tokio::net::TcpStream::connect(authority).await.unwrap();
            streaming
                .write_all(format!(
                    "POST /api/auth/passkey/register/begin HTTP/1.1\r\nHost: {authority}\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nContent-Length: 1000\r\nExpect: 100-continue\r\n\r\n"
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
            let coords: crate::runtime_control::RuntimeCoords =
                serde_json::from_slice(&std::fs::read(&coords_path).unwrap()).unwrap();
            assert_eq!(coords.home_url, home);
            assert_eq!(
                coords.binary_sha256,
                crate::runtime_control::sha256_file(&std::env::current_exe().unwrap()).unwrap()
            );
            let control_addr = coords.api_url.strip_prefix("http://").unwrap();
            let mut control_stream = tokio::net::TcpStream::connect(control_addr).await.unwrap();
            control_stream.write_all(format!("POST /api/auth/attach HTTP/1.1\r\nHost: {control_addr}\r\nContent-Type: application/json\r\nContent-Length: 1000\r\nExpect: 100-continue\r\n\r\n").as_bytes()).await.unwrap();
            let length =
                tokio::time::timeout(Duration::from_secs(2), control_stream.read(&mut interim))
                    .await
                    .unwrap()
                    .unwrap();
            assert!(String::from_utf8_lossy(&interim[..length]).starts_with("HTTP/1.1 100 Continue"));

            (
                streaming,
                control_stream,
                interim,
                coords_path,
                managed_pid,
                managed_helper_pid,
                unrelated,
            )
        })
        .await;

        let _ = stop_tx.send(());
        let mut server = server;
        let stopped = tokio::time::timeout(Duration::from_secs(5), &mut server).await;
        if stopped.is_err() {
            server.abort();
            let _ = server.await;
            panic!("gateway shutdown waited for a client stream to close");
        }
        stopped.unwrap().unwrap().unwrap();
        let (
            mut streaming,
            mut control_stream,
            mut interim,
            coords_path,
            managed_pid,
            managed_helper_pid,
            unrelated,
        ) = journey.unwrap();
        let control_closed =
            tokio::time::timeout(Duration::from_secs(1), control_stream.read(&mut interim))
                .await
                .unwrap();
        assert!(matches!(control_closed, Ok(0)) || control_closed.is_err());
        assert_eq!(unsafe { libc::kill(managed_pid as i32, 0) }, -1);
        assert_eq!(unsafe { libc::kill(managed_helper_pid as i32, 0) }, -1);
        assert_eq!(unsafe { libc::kill(unrelated.child.id() as i32, 0) }, 0);
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
        assert!(!crate::runtime_control::gateway_runtime_coord_path(temp.path()).exists());
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
        assert!(!crate::runtime_control::gateway_runtime_coord_path(temp.path()).exists());
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
