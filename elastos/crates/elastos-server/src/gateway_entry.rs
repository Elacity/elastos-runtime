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
        setup_gateway_control_plane,
    )
    .await
}

pub async fn run_browser_home() -> anyhow::Result<()> {
    elastos_server::update_controller::enter_browser_home().await?;
    elastos_server::gateway_cmd::run_gateway_direct_with_ready(
        format!(
            "localhost:{}",
            elastos_server::update_controller::BROWSER_HOME_PORT
        ),
        false,
        None,
        None,
        setup_control_plane,
        Some(browser_home_ready),
    )
    .await
}

fn browser_home_ready(url: &str) {
    if std::env::var("ELASTOS_UPDATE_RESTART").as_deref() == Ok("1")
        || elastos_server::home_service::service_mode()
    {
        return;
    }
    eprintln!("Home: {url}");
    eprintln!("Keep this terminal open. Press Ctrl+C to stop Home.");
    crate::open_browser(url);
}

/// A second `elastos home --browser` opens the Home that already runs, or starts
/// Home's background service. Returns false when Home should start here as before.
pub async fn reopen_running_home() -> anyhow::Result<bool> {
    if elastos_server::update_controller::is_controller_host() {
        return Ok(false);
    }
    let data_dir = crate::sources::default_data_dir();
    let Some(url) = elastos_server::home_service::running_home_url(&data_dir).await? else {
        return Ok(false);
    };
    println!("Home: {url}");
    crate::open_browser(&url);
    Ok(true)
}

async fn setup_control_plane() -> anyhow::Result<elastos_server::gateway_cmd::GatewayControlPlane> {
    let infra = crate::server_infra::setup_control_plane_infrastructure().await?;
    Ok(control_plane(infra))
}

async fn setup_gateway_control_plane(
) -> anyhow::Result<elastos_server::gateway_cmd::GatewayControlPlane> {
    let infra = crate::server_infra::setup_gateway_infrastructure().await?;
    Ok(control_plane(infra))
}

fn control_plane(
    infra: crate::server_infra::ServerInfrastructure,
) -> elastos_server::gateway_cmd::GatewayControlPlane {
    elastos_server::gateway_cmd::GatewayControlPlane {
        provider_registry: infra.provider_registry,
        host_helpers: infra.host_helpers,
        carrier_service: infra.carrier_service,
        collaboration_context: infra.collaboration_context,
        collaboration_service: infra.collaboration_service,
    }
}
