use super::*;
use std::env;

pub(super) fn default_networks() -> Vec<ChainNetwork> {
    vec![
        ChainNetwork {
            id: "ela-mainnet".to_string(),
            display_name: "Elastos Mainchain".to_string(),
            kind: ChainKind::MainchainRest,
            chain_id: None,
            native_symbol: "ELA".to_string(),
            provider: "Elastos".to_string(),
            mainnet: true,
            explorer_url: Some("https://blockchain.elastos.io".to_string()),
            rpc_url: "https://blockchain.elastos.io/api/v1".to_string(),
            rights_methods: Vec::new(),
        },
        ChainNetwork {
            id: "esc-mainnet".to_string(),
            display_name: "Elastos Smart Chain".to_string(),
            kind: ChainKind::EvmJsonRpc,
            chain_id: Some(20),
            native_symbol: "ELA".to_string(),
            provider: "Elastos".to_string(),
            mainnet: true,
            explorer_url: Some("https://esc.elastos.io".to_string()),
            rpc_url: "https://api.elastos.io/esc".to_string(),
            rights_methods: Vec::new(),
        },
        ChainNetwork {
            id: "base-mainnet".to_string(),
            display_name: "Base".to_string(),
            kind: ChainKind::EvmJsonRpc,
            chain_id: Some(8453),
            native_symbol: "ETH".to_string(),
            provider: "Base".to_string(),
            mainnet: true,
            explorer_url: Some("https://basescan.org".to_string()),
            rpc_url: env::var("BASE_RPC_URL")
                .unwrap_or_else(|_| "https://mainnet.base.org".to_string()),
            rights_methods: Vec::new(),
        },
        ChainNetwork {
            id: "btc-mainnet".to_string(),
            display_name: "Bitcoin".to_string(),
            kind: ChainKind::BitcoinRest,
            chain_id: None,
            native_symbol: "BTC".to_string(),
            provider: "mempool.space".to_string(),
            mainnet: true,
            explorer_url: Some("https://mempool.space".to_string()),
            rpc_url: env::var("BITCOIN_REST_URL")
                .unwrap_or_else(|_| "https://mempool.space/api".to_string()),
            rights_methods: Vec::new(),
        },
    ]
}

pub(super) fn validate_networks(networks: &[ChainNetwork]) -> Result<(), String> {
    if networks.is_empty() {
        return Err("at least one network is required".to_string());
    }
    for network in networks {
        validate_network_id(&network.id)?;
        if network.display_name.trim().is_empty() {
            return Err("network display name is required".to_string());
        }
        validate_rpc_url(network)?;
        if network.kind == ChainKind::EvmJsonRpc && network.chain_id.is_none() {
            return Err(format!("EVM network {} requires chain_id", network.id));
        }
        if network.kind != ChainKind::EvmJsonRpc && !network.rights_methods.is_empty() {
            return Err(format!(
                "network {} cannot configure EVM rights methods",
                network.id
            ));
        }
        validate_rights_methods(network)?;
    }
    Ok(())
}

pub(super) fn validate_rights_methods(network: &ChainNetwork) -> Result<(), String> {
    let mut seen_actions = std::collections::BTreeSet::new();
    for method in &network.rights_methods {
        if method.id != "has_access_by_content_id" {
            return Err(format!("unsupported rights method id: {}", method.id));
        }
        validate_evm_address(&method.contract)?;
        validate_hex(&method.selector, Some(4), "EVM function selector")?;
        if method.protected_content_policies.len() > 4 {
            return Err(format!(
                "network {} configures too many protected-content policy sources",
                network.id
            ));
        }
        for policy in &method.protected_content_policies {
            if policy.right_argument != policy.action.expected_right_argument() {
                return Err(format!(
                    "network {} configures a mismatched protected-content policy right for {:?}",
                    network.id, policy.action
                ));
            }
            if !seen_actions.insert(policy.action) {
                return Err(format!(
                    "network {} configures duplicate protected-content policy action {:?}",
                    network.id, policy.action
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_rpc_url(network: &ChainNetwork) -> Result<(), String> {
    let url = network.rpc_url.trim();
    if url_has_userinfo(url) {
        return Err(format!("invalid RPC URL for {}", network.id));
    }
    if network.kind == ChainKind::BitcoinCoreRpc {
        if url.is_empty()
            || url.starts_with("http://127.0.0.1:")
            || url.starts_with("http://localhost:")
        {
            return Ok(());
        }
        return Err(format!(
            "Bitcoin Core RPC URL for {} must be empty or loopback HTTP",
            network.id
        ));
    }
    if url.is_empty()
        || !(url.starts_with("https://")
            || url.starts_with("http://127.0.0.1:")
            || url.starts_with("http://localhost:"))
    {
        return Err(format!("invalid RPC URL for {}", network.id));
    }
    Ok(())
}

fn url_has_userinfo(url: &str) -> bool {
    let Some((_, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    authority.contains('@')
}
