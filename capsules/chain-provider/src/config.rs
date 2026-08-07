use super::*;
use std::env;

/// PC2's smoke-tested Base RPC pool (`src/utils/rpc.ts` / `config/default.json`): key-less,
/// rate-tolerant public endpoints, round-robin with failover. Index 0 MUST stay key-less and
/// rate-tolerant — a keyed provider at the head that exhausts quota silently degrades the
/// rights read to "not owned" (the May 2026 PC2 video-playback incident).
pub(super) const PC2_BASE_RPC_POOL: &[&str] = &[
    "https://base-rpc.publicnode.com",
    "https://base.drpc.org",
    "https://mainnet.base.org",
    "https://base-mainnet.public.blastapi.io",
    "https://base.meowrpc.com",
    "https://1rpc.io/base",
];

/// The subset of the Base pool that CORRECTLY serves `eth_getLogs` over a 10k-block range.
/// Probed Jun 2026 against a known channel mid-window: only `mainnet.base.org` (official) and
/// `base.gateway.tenderly.co` return the real logs. CRITICAL: `publicnode` is EXCLUDED — it
/// SILENTLY TRUNCATES, returning HTTP 200 with `[]` for ranges it won't fully scan (a false
/// "no channels", worse than an error). `drpc` free-tier times out (HTTP 408) at 10k, and
/// `blastapi`/`meowrpc`/`1rpc.io` cap or refuse `eth_getLogs`. So channel discovery routes
/// ONLY here — a lying endpoint can never be the one whose empty answer we trust (#11).
pub(super) const PC2_BASE_LOG_RPC_POOL: &[&str] = &[
    "https://mainnet.base.org",
    "https://base.gateway.tenderly.co",
];

fn operator_base_head() -> Option<String> {
    env::var("ELASTOS_CHAIN_BASE_RPC")
        .or_else(|_| env::var("BASE_RPC_URL"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The effective Base pool: an operator-supplied head (`ELASTOS_CHAIN_BASE_RPC`, else the
/// legacy `BASE_RPC_URL`) prepended to the public pool as failover, else the public pool
/// as-is. Returns `(primary, fallbacks)` with no duplicates.
fn base_rpc_pool() -> (String, Vec<String>) {
    let mut ordered: Vec<String> = Vec::new();
    if let Some(head) = operator_base_head() {
        ordered.push(head);
    }
    for url in PC2_BASE_RPC_POOL {
        let url = url.to_string();
        if !ordered.iter().any(|u| u == &url) {
            ordered.push(url);
        }
    }
    let primary = ordered.remove(0);
    (primary, ordered)
}

/// The Base `eth_getLogs` pool: the operator head (assumed range-capable since the operator
/// chose it) followed by the probed range-capable publics, de-duplicated. Channel discovery
/// routes log queries here so a strict public endpoint can never break the factory scan.
fn base_log_rpc_pool() -> Vec<String> {
    let mut ordered: Vec<String> = Vec::new();
    if let Some(head) = operator_base_head() {
        ordered.push(head);
    }
    for url in PC2_BASE_LOG_RPC_POOL {
        let url = url.to_string();
        if !ordered.iter().any(|u| u == &url) {
            ordered.push(url);
        }
    }
    ordered
}

pub(super) fn default_networks() -> Vec<ChainNetwork> {
    let (base_rpc, base_fallbacks) = base_rpc_pool();
    let base_log_rpcs = base_log_rpc_pool();
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
            rpc_fallback_urls: Vec::new(),
            log_query_rpc_urls: Vec::new(),
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
            rpc_fallback_urls: Vec::new(),
            log_query_rpc_urls: Vec::new(),
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
            rpc_url: base_rpc,
            rpc_fallback_urls: base_fallbacks,
            log_query_rpc_urls: base_log_rpcs,
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
            rpc_fallback_urls: Vec::new(),
            log_query_rpc_urls: Vec::new(),
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
        for fallback in &network.rpc_fallback_urls {
            let probe = ChainNetwork {
                rpc_url: fallback.clone(),
                ..network.clone()
            };
            validate_rpc_url(&probe)
                .map_err(|_| format!("invalid fallback RPC URL for {}", network.id))?;
        }
        for log_rpc in &network.log_query_rpc_urls {
            let probe = ChainNetwork {
                rpc_url: log_rpc.clone(),
                ..network.clone()
            };
            validate_rpc_url(&probe)
                .map_err(|_| format!("invalid log-query RPC URL for {}", network.id))?;
        }
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
    for method in &network.rights_methods {
        if method.id != "has_access_by_content_id" {
            return Err(format!("unsupported rights method id: {}", method.id));
        }
        validate_evm_address(&method.contract)?;
        validate_hex(&method.selector, Some(4), "EVM function selector")?;
    }
    Ok(())
}

pub(super) fn validate_rpc_url(network: &ChainNetwork) -> Result<(), String> {
    let url = network.rpc_url.trim();
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
