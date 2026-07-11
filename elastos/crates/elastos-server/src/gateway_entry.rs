pub async fn run_gateway(
    addr: String,
    public: bool,
    cache_dir: Option<std::path::PathBuf>,
    publish: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    elastos_server::gateway_cmd::run_gateway_direct(addr, public, cache_dir, publish, || async {
        let infra = crate::server_infra::setup_control_plane_infrastructure().await?;
        Ok(elastos_server::gateway_cmd::GatewayControlPlane {
            provider_registry: infra.provider_registry,
            host_helpers: infra.host_helpers,
            standing_service: Some(infra.standing_service),
            capability_manager: Some(infra.capability_manager),
            pay_rail: infra.pay_rail,
        })
    })
    .await
}
