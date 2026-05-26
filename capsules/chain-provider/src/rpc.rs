use super::*;
use std::env;

pub(super) fn backend_url(network: &ChainNetwork, path: &str) -> Result<String, Response> {
    let base = network.rpc_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(Response::error(
            "backend_not_configured",
            &format!("backend URL is not configured for {}", network.id),
        ));
    }
    Ok(format!("{}/{}", base, path.trim_start_matches('/')))
}

pub(super) fn bitcoin_rpc_auth(_network_id: &str) -> Option<(String, String)> {
    let user = env::var("BITCOIN_RPC_USER").ok()?;
    let password = env::var("BITCOIN_RPC_PASSWORD").ok()?;
    if user.trim().is_empty() || password.is_empty() {
        return None;
    }
    Some((user, password))
}
