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
            protected_content_creator_mint: None,
            protected_content_market: None,
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
            protected_content_creator_mint: None,
            protected_content_market: None,
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
            protected_content_creator_mint: None,
            protected_content_market: None,
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
            protected_content_creator_mint: None,
            protected_content_market: None,
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
        if network.kind != ChainKind::EvmJsonRpc && network.protected_content_creator_mint.is_some()
        {
            return Err(format!(
                "network {} cannot configure protected-content creator mint on a non-EVM backend",
                network.id
            ));
        }
        if network.kind != ChainKind::EvmJsonRpc && network.protected_content_market.is_some() {
            return Err(format!(
                "network {} cannot configure protected-content market on a non-EVM backend",
                network.id
            ));
        }
        validate_rights_methods(network)?;
        validate_protected_content_creator_mint(network)?;
        validate_protected_content_market(network)?;
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
            if !seen_actions.insert(policy.action) {
                return Err(format!(
                    "network {} configures duplicate protected-content policy action {:?}",
                    network.id, policy.action
                ));
            }
            validate_protected_content_policy_sources(network, policy)?;
        }
    }
    Ok(())
}

fn validate_protected_content_creator_mint(network: &ChainNetwork) -> Result<(), String> {
    let Some(mint) = network.protected_content_creator_mint.as_ref() else {
        return Ok(());
    };
    validate_evm_address(&mint.ledger)?;
    validate_evm_address(&mint.pay_token)?;
    validate_evm_address(&mint.asset_created_emitter)?;
    Ok(())
}

fn validate_protected_content_market(network: &ChainNetwork) -> Result<(), String> {
    let Some(market) = network.protected_content_market.as_ref() else {
        return Ok(());
    };
    validate_evm_address(&market.authority_gateway_contract)?;
    validate_protected_content_policy_sources(
        network,
        &ProtectedContentPolicySource {
            action: ProtectedContentPolicyAction::View,
            evidence_rpc_urls: market.evidence_rpc_urls.clone(),
        },
    )?;
    Ok(())
}

pub(super) fn validate_rpc_url(network: &ChainNetwork) -> Result<(), String> {
    validate_rpc_url_value(
        &network.id,
        &network.rpc_url,
        network.kind == ChainKind::BitcoinCoreRpc,
    )
}

fn url_has_userinfo(url: &str) -> bool {
    let Some((_, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    authority.contains('@')
}

fn validate_protected_content_policy_sources(
    network: &ChainNetwork,
    policy: &ProtectedContentPolicySource,
) -> Result<(), String> {
    if !(2..=5).contains(&policy.evidence_rpc_urls.len()) {
        return Err(format!(
            "network {} action {:?} must configure 2-5 protected-content evidence RPC URLs",
            network.id, policy.action
        ));
    }
    let mut unique_urls = std::collections::BTreeSet::new();
    for url in &policy.evidence_rpc_urls {
        let canonical = canonicalize_protected_content_evidence_rpc_url(&network.id, url)?;
        if !unique_urls.insert(canonical) {
            return Err(format!(
                "network {} action {:?} configures duplicate protected-content evidence RPC URLs",
                network.id, policy.action
            ));
        }
    }
    Ok(())
}

fn canonicalize_protected_content_evidence_rpc_url(
    network_id: &str,
    url: &str,
) -> Result<String, String> {
    let trimmed = url.trim();
    validate_rpc_url_value(network_id, trimmed, false)?;
    Ok(trimmed.trim_end_matches('/').to_string())
}

fn validate_rpc_url_value(
    network_id: &str,
    url: &str,
    allow_empty_loopback_only: bool,
) -> Result<(), String> {
    if url_has_userinfo(url) {
        return Err(format!("invalid RPC URL for {}", network_id));
    }
    if allow_empty_loopback_only {
        if url.is_empty()
            || url.starts_with("http://127.0.0.1:")
            || url.starts_with("http://localhost:")
        {
            return Ok(());
        }
        return Err(format!(
            "Bitcoin Core RPC URL for {} must be empty or loopback HTTP",
            network_id
        ));
    }
    if url.is_empty()
        || !(url.starts_with("https://")
            || url.starts_with("http://127.0.0.1:")
            || url.starts_with("http://localhost:"))
    {
        return Err(format!("invalid RPC URL for {}", network_id));
    }
    Ok(())
}
