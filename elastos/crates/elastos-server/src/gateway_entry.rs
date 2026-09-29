pub async fn run_gateway(
    addr: String,
    public: bool,
    cache_dir: Option<std::path::PathBuf>,
    publish: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    elastos_server::gateway_cmd::run_gateway_direct(
        addr,
        public,
        cache_dir,
        publish,
        setup_control_plane,
    )
    .await
}

pub async fn run_browser_home() -> anyhow::Result<()> {
    elastos_server::gateway_cmd::run_gateway_direct_with_ready(
        "localhost:8090".to_string(),
        false,
        None,
        None,
        setup_control_plane,
        Some(browser_home_ready),
    )
    .await
}

fn browser_home_ready(url: &str) {
    eprintln!("Home: {url}");
    eprintln!("Keep this terminal open. Press Ctrl+C to stop Home.");
    crate::open_browser(url);
}

async fn setup_control_plane() -> anyhow::Result<elastos_server::gateway_cmd::GatewayControlPlane> {
    let infra = crate::server_infra::setup_control_plane_infrastructure().await?;
    Ok(elastos_server::gateway_cmd::GatewayControlPlane {
        provider_registry: infra.provider_registry,
        host_helpers: infra.host_helpers,
        carrier_service: infra.carrier_service,
        collaboration_context: infra.collaboration_context,
        collaboration_service: infra.collaboration_service,
    })
}
