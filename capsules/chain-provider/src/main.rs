//! ElastOS Chain Provider Capsule
//!
//! Typed chain access for Elastos and node-backed networks.
//! Apps never receive raw RPC URLs or arbitrary JSON-RPC passthrough.

use elastos_guest::prelude::*;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

mod abi;
mod backends;
mod channel_index;
mod config;
mod lifecycle;
mod protocol;
mod rpc;
mod validation;

#[cfg(test)]
mod tests;

use abi::*;
use channel_index::*;
use config::*;
use lifecycle::*;
use protocol::*;
use rpc::*;
use validation::*;

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const NODE_LIFECYCLE_CONTROL_REASON: &str =
    "node lifecycle control requires an operator-approved supervisor";

/// The real Base channel factory (`config/default.json` `contracts.v3.channel_factory`).
/// Default for channel discovery + createChannel; overridable per request.
const DEFAULT_CHANNEL_FACTORY: &str = "0xE1365ed47353De2F8A6a69E271e36650A9EE368F";
/// `keccak256("ChannelCreated(uint8,uint8,address,address,address)")` — the event topic the
/// factory emits per channel. Pinned (not computed in-capsule), like the call selectors.
const CHANNEL_CREATED_TOPIC0: &str =
    "0x4ae6ef95ddade103ca67593cd4cf68dda177aa1054ad4eeb4963d2c3df44702e";
/// The Base block the channel factory was deployed at (`contracts.v3.from_block`); the
/// default lower bound for the `ChannelCreated` scan.
const DEFAULT_CHANNEL_FROM_BLOCK: u64 = 43_892_000;
/// `keccak256("AssetCreated(address,address,uint256,string,uint16,address)")` — the event a
/// successful mint emits (topic1=_to, topic2=_channel, topic3 indexed opContract). Pinned
/// (not computed in-capsule), like the call selectors; used to discover the just-minted
/// asset's operative contract for the trade-enabling approval.
const ASSET_CREATED_TOPIC0: &str =
    "0xc0a995e4052be044599af577ab2f3382d67bd34df95a76226e7c464e9d4dba46";

/// From a transaction receipt's `logs` array, return the NEWEST (by `logIndex`) `AssetCreated`
/// `(operative, token_id_hex)` whose topics match `(creator_topic, channel_topic)`. The receipt's
/// logs are that ONE transaction's own, so unlike the `eth_getLogs` scan (which pre-filters in the
/// query) we topic-match each log here. Pure (no RPC) so it is unit-testable; `None` if no matching
/// `AssetCreated` is present.
fn newest_asset_created_in_logs(
    logs: &[Value],
    creator_topic: &str,
    channel_topic: &str,
) -> Option<(String, String)> {
    let mut best: Option<(String, String, u64)> = None;
    for log in logs {
        let Some(topics) = log.get("topics").and_then(Value::as_array) else {
            continue;
        };
        let t0 = topics.first().and_then(Value::as_str).unwrap_or_default();
        let t1 = topics.get(1).and_then(Value::as_str).unwrap_or_default();
        let t2 = topics.get(2).and_then(Value::as_str).unwrap_or_default();
        if !t0.eq_ignore_ascii_case(ASSET_CREATED_TOPIC0)
            || !t1.eq_ignore_ascii_case(creator_topic)
            || !t2.eq_ignore_ascii_case(channel_topic)
        {
            continue;
        }
        let Some((operative, token_id, _block, log_index)) = decode_asset_created_log(log) else {
            continue;
        };
        if best
            .as_ref()
            .map(|(_, _, l)| log_index > *l)
            .unwrap_or(true)
        {
            best = Some((operative, token_id, log_index));
        }
    }
    best.map(|(operative, token_id, _)| (operative, token_id))
}
/// `authority()` selector — reads the channel's gateway (the operative authority). Pinned.
const AUTHORITY_SELECTOR: &str = "0xbf7e214f";
/// `isApprovedForAll(address,address)` selector — reads whether the gateway is already an
/// approved operator on an operative contract (idempotency check). Pinned.
const IS_APPROVED_FOR_ALL_SELECTOR: &str = "0xe985e9c5";
/// `setApprovalForAll(address,bool)` selector — pinned, handed to the pure assembler.
const SET_APPROVAL_FOR_ALL_SELECTOR: &str = "0xa22cb465";
/// The real Base AuthorityGateway (`config/default.json` `contracts.v3.authority_gateway`),
/// the fallback gateway when a channel's `authority()` read misses. The app never names it.
const DEFAULT_AUTHORITY_GATEWAY: &str = "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D";
/// Max block span per `eth_getLogs` window. PINNED to 10k — the proven-safe span across the
/// whole Base log pool (`pc2-node config/default.json` `max_blocks_per_scan: 10000`). Probed
/// Jun 2026: `drpc` and `mainnet.base.org` HARD-CAP at 10k (HTTP 400/413 over it) and
/// `publicnode` is unreliable on wide ranges (rate-limits → empty 200 OR errors), so a larger
/// span silently dropped channels or failed closed. The scanner is still ADAPTIVE — it halves
/// on any "range too large" signal (JSON code or HTTP 400/413) — but 10k means it normally
/// never has to split, matching PC2's behaviour exactly.
const DEFAULT_MAX_LOG_RANGE: u64 = 10_000;
/// The scanner never splits a window below this many blocks — a "range" error at/below this
/// is a real failure (not a range cap), so we fail closed instead of looping forever.
const MIN_LOG_RANGE: u64 = 2_000;
/// Backfill budget per `list_channels` call: how many top-level windows we scan downward
/// before returning, so a single (synchronous) call stays responsive. The cursor is persisted,
/// so coverage resumes on the next call — newest-first, so recent channels surface first.
const DEFAULT_BACKFILL_WINDOWS_PER_CALL: u64 = 16;
/// How many newest-first `max_log_range` windows `assemble_trade_approval` scans for the
/// creator's `AssetCreated` log before giving up. 48 × 10k ≈ 480k Base blocks (~11 days at
/// ~2s/block) — generous for a "just minted, now enable trading" flow (which early-stops in
/// the first window or two) while keeping a single synchronous call bounded.
const TRADE_APPROVAL_SCAN_WINDOWS: u64 = 48;

struct ChainProvider {
    networks: Vec<ChainNetwork>,
    client: reqwest::blocking::Client,
    node_lifecycle_state_path: PathBuf,
    node_lifecycle_state: NodeLifecycleStateFile,
    node_lifecycle_state_error: Option<String>,
    node_supervisor: NodeSupervisorConfig,
    channel_index_path: PathBuf,
    channel_index: ChannelIndexFile,
}

impl ChainProvider {
    fn new() -> Self {
        Self::with_data_dir(data_dir())
    }

    fn with_data_dir(data_dir: PathBuf) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("chain-provider HTTP client should initialize");
        let node_lifecycle_state_path = node_lifecycle_state_path(&data_dir);
        let (node_lifecycle_state, node_lifecycle_state_error) =
            match read_node_lifecycle_state_file(&node_lifecycle_state_path) {
                Ok(state) => (state, None),
                Err(err) => (NodeLifecycleStateFile::default(), Some(err)),
            };
        let channel_index_path = channel_index_path(&data_dir);
        // A corrupt index is a recoverable cache, not authority: start empty and let the
        // next scan repopulate it (fail-open for a cache, never for ownership).
        let channel_index = read_channel_index_file(&channel_index_path).unwrap_or_default();
        Self {
            networks: default_networks(),
            client,
            node_lifecycle_state_path,
            node_lifecycle_state,
            node_lifecycle_state_error,
            node_supervisor: NodeSupervisorConfig::default(),
            channel_index_path,
            channel_index,
        }
    }

    fn handle(&mut self, req: Request) -> Response {
        match req {
            Request::Init { config } => self.init(config),
            Request::Networks => Response::ok(json!({
                "networks": self.networks.iter().map(ChainNetwork::public_view).collect::<Vec<_>>()
            })),
            Request::Status { network } => self.status(&network),
            Request::BlockNumber { network } => self.block_number(&network),
            Request::SyncHealth { network } => self.sync_health(&network),
            Request::Balance {
                network,
                address,
                block,
            } => self.balance(&network, &address, block.as_deref()),
            Request::ContractCall {
                network,
                to,
                data,
                block,
            } => self.contract_call(&network, &to, &data, block.as_deref()),
            Request::EstimateGas {
                network,
                from,
                to,
                value,
                data,
            } => self.estimate_gas(
                &network,
                &from,
                &to,
                value.as_deref().unwrap_or("0x0"),
                data.as_deref().unwrap_or("0x"),
            ),
            Request::TransactionCount {
                network,
                address,
                block,
            } => self.transaction_count(&network, &address, block.as_deref()),
            Request::GasPrice { network } => self.gas_price(&network),
            Request::FeeHistory {
                network,
                block_count,
                newest_block,
                reward_percentiles,
            } => self.fee_history(&network, &block_count, &newest_block, &reward_percentiles),
            Request::Code {
                network,
                address,
                block,
            } => self.code(&network, &address, block.as_deref()),
            Request::Logs { network, filter } => self.logs(&network, filter),
            Request::Transaction { network, hash } => self.transaction(&network, &hash),
            Request::Receipt { network, hash } => self.receipt(&network, &hash),
            Request::HasAccessByContentId {
                network,
                contract,
                content_id,
                subject,
                right,
            } => self.has_access_by_content_id(&network, &contract, &content_id, &subject, &right),
            Request::Proof {
                network,
                proof_kind,
                subject,
            } => self.proof(&network, proof_kind, &subject),
            Request::Erc1271IsValidSignature {
                network,
                contract,
                message_hash,
                signature,
            } => self.erc1271_is_valid_signature(&network, &contract, &message_hash, &signature),
            Request::PrepareTransaction {
                network,
                from,
                to,
                value,
                data,
            } => self.prepare_transaction(&network, &from, &to, &value, data.as_deref()),
            Request::BroadcastTransaction {
                network,
                signed_transaction,
            } => self.broadcast_transaction(&network, &signed_transaction),
            Request::NodeLifecycle { network, action } => self.node_lifecycle(&network, action),
            Request::AssembleMint { mint } => self.assemble_mint(*mint),
            Request::ListChannels {
                network,
                factory,
                creator,
                from_block,
            } => self.list_channels(
                &network,
                factory.as_deref(),
                &creator,
                from_block.as_deref(),
            ),
            Request::ResolveTokenId {
                network,
                ledger,
                content_id,
                from_block,
            } => self.resolve_token_id(&network, &ledger, &content_id, from_block.as_deref()),
            Request::AssembleCreateChannel { channel } => self.assemble_create_channel(*channel),
            Request::AssembleTradeApproval {
                network,
                channel,
                creator,
                content_id,
                tx_hash,
            } => self.assemble_trade_approval(
                &network,
                &channel,
                &creator,
                content_id.as_deref(),
                tx_hash.as_deref(),
            ),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, config: Value) -> Response {
        let extra = config.get("extra").unwrap_or(&config);
        if let Some(networks) = config
            .get("extra")
            .and_then(|extra| extra.get("networks"))
            .or_else(|| config.get("networks"))
        {
            match serde_json::from_value::<Vec<ChainNetwork>>(networks.clone()) {
                Ok(networks) => {
                    if let Err(err) = validate_networks(&networks) {
                        return Response::error("invalid_config", &err);
                    }
                    self.networks = networks;
                }
                Err(err) => return Response::error("invalid_config", &err.to_string()),
            }
        }
        if let Some(supervisor) = extra.get("node_supervisor") {
            match serde_json::from_value::<NodeSupervisorConfig>(supervisor.clone()) {
                Ok(supervisor) => {
                    if let Err(err) = validate_node_supervisor_config(&supervisor) {
                        return Response::error("invalid_config", &err);
                    }
                    self.node_supervisor = supervisor;
                }
                Err(err) => return Response::error("invalid_config", &err.to_string()),
            }
        }
        Response::ok(json!({
            "provider": "chain",
            "protocol_version": "1.0",
            "network_count": self.networks.len(),
        }))
    }

    fn status(&self, network_id: &str) -> Response {
        let network = match self.network_for_status(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        match network.kind {
            ChainKind::EvmJsonRpc => self.evm_status(network),
            ChainKind::BitcoinCoreRpc => self.bitcoin_status(network),
            ChainKind::BitcoinRest => self.bitcoin_rest_status(network),
            ChainKind::MainchainRest => self.mainchain_status(network),
        }
    }

    fn evm_status(&self, network: &ChainNetwork) -> Response {
        let chain_id = match self.evm_rpc(network, "eth_chainId", json!([])) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let block_number = match self.evm_rpc(network, "eth_blockNumber", json!([])) {
            Ok(value) => value,
            Err(response) => return response,
        };
        if let Some(expected) = network.chain_id {
            match parse_hex_u64(chain_id.as_str().unwrap_or_default()) {
                Ok(actual) if actual == expected => {}
                Ok(actual) => {
                    return Response::error(
                        "chain_id_mismatch",
                        &format!(
                            "upstream chain id {} does not match configured chain id {}",
                            actual, expected
                        ),
                    );
                }
                Err(err) => return Response::error("invalid_upstream_chain_id", &err),
            }
        }
        Response::ok(json!({
            "network": network.public_view(),
            "chain_id_hex": chain_id,
            "block_number_hex": block_number,
            "block_number": block_number.as_str().and_then(|value| parse_hex_u64(value).ok()),
        }))
    }

    fn block_number(&self, network_id: &str) -> Response {
        match self.network_for_status(network_id) {
            Ok(network) => match network.kind {
                ChainKind::EvmJsonRpc => {
                    match self.evm_rpc(network, "eth_blockNumber", json!([])) {
                        Ok(block_number) => Response::ok(json!({
                            "network": network.id,
                            "block_number_hex": block_number,
                            "block_number": block_number.as_str().and_then(|value| parse_hex_u64(value).ok()),
                        })),
                        Err(response) => response,
                    }
                }
                ChainKind::BitcoinCoreRpc => {
                    match self.bitcoin_rpc(network, "getblockcount", json!([])) {
                        Ok(block_height) => Response::ok(json!({
                            "network": network.id,
                            "block_height": block_height.as_u64(),
                        })),
                        Err(response) => response,
                    }
                }
                ChainKind::BitcoinRest => match self.bitcoin_rest_tip_height(network) {
                    Ok(block_height) => Response::ok(json!({
                        "network": network.id,
                        "block_height": block_height,
                    })),
                    Err(response) => response,
                },
                ChainKind::MainchainRest => match self.mainchain_tip(network) {
                    Ok(tip) => Response::ok(json!({
                        "network": network.id,
                        "block_height": tip.height,
                    })),
                    Err(response) => response,
                },
            },
            Err(response) => response,
        }
    }

    fn balance(&self, network_id: &str, address: &str, block: Option<&str>) -> Response {
        let network = match self.network_for_status(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        match network.kind {
            ChainKind::EvmJsonRpc => self.evm_balance(network, address, block),
            ChainKind::BitcoinRest => self.bitcoin_rest_balance(network, address),
            ChainKind::BitcoinCoreRpc => Response::error(
                "unsupported_network_kind",
                "Bitcoin Core arbitrary address balances are not exposed through this provider",
            ),
            ChainKind::MainchainRest => Response::error(
                "unsupported_network_kind",
                "this operation currently supports EVM balances and Bitcoin REST balances only",
            ),
        }
    }

    fn evm_balance(&self, network: &ChainNetwork, address: &str, block: Option<&str>) -> Response {
        if let Err(err) = validate_evm_address(address) {
            return Response::error("invalid_address", &err);
        }
        let block = match normalize_block_tag(block) {
            Ok(block) => block,
            Err(err) => return Response::error("invalid_block", &err),
        };
        match self.evm_rpc(network, "eth_getBalance", json!([address, block])) {
            Ok(balance) => Response::ok(json!({
                "network": network.id,
                "address": address,
                "block": block,
                "balance_hex": balance,
                "native_symbol": network.native_symbol,
            })),
            Err(response) => response,
        }
    }

    fn bitcoin_rest_balance(&self, network: &ChainNetwork, address: &str) -> Response {
        if let Err(err) = validate_bitcoin_rest_address(address) {
            return Response::error("invalid_address", &err);
        }
        let body = match self.backend_get_json(network, &format!("address/{address}")) {
            Ok(body) => body,
            Err(response) => return response,
        };
        let confirmed = match bitcoin_balance_sats(&body, "chain_stats") {
            Ok(value) => value,
            Err(err) => return Response::error("upstream_invalid_balance", &err),
        };
        let mempool = match bitcoin_balance_sats(&body, "mempool_stats") {
            Ok(value) => value,
            Err(err) => return Response::error("upstream_invalid_balance", &err),
        };
        Response::ok(json!({
            "network": network.id,
            "address": address,
            "balance_sats": confirmed.saturating_add(mempool),
            "confirmed_sats": confirmed,
            "mempool_sats": mempool,
            "native_symbol": network.native_symbol,
        }))
    }

    fn contract_call(
        &self,
        network_id: &str,
        to: &str,
        data: &str,
        block: Option<&str>,
    ) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_address(to) {
            return Response::error("invalid_to", &err);
        }
        if let Err(err) = validate_hex(data, None, "call data") {
            return Response::error("invalid_data", &err);
        }
        if data.len() > 256 * 1024 {
            return Response::error("invalid_data", "call data is too large");
        }
        let block = match normalize_block_tag(block) {
            Ok(block) => block,
            Err(err) => return Response::error("invalid_block", &err),
        };
        match self.evm_rpc(
            network,
            "eth_call",
            json!([{ "to": to, "data": data }, block]),
        ) {
            Ok(result) => Response::ok(json!({
                "schema": "elastos.chain.contract_call/v1",
                "network": network.id,
                "to": to,
                "data": data,
                "block": block,
                "result": result,
            })),
            Err(response) => response,
        }
    }

    fn estimate_gas(
        &self,
        network_id: &str,
        from: &str,
        to: &str,
        value: &str,
        data: &str,
    ) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_address(from) {
            return Response::error("invalid_from", &err);
        }
        if let Err(err) = validate_evm_address(to) {
            return Response::error("invalid_to", &err);
        }
        if let Err(err) = validate_hex_quantity(value, "value") {
            return Response::error("invalid_value", &err);
        }
        if let Err(err) = validate_hex(data, None, "transaction data") {
            return Response::error("invalid_data", &err);
        }
        if data.len() > 256 * 1024 {
            return Response::error("invalid_data", "transaction data is too large");
        }
        match self.evm_rpc(
            network,
            "eth_estimateGas",
            json!([{ "from": from, "to": to, "value": value, "data": data }]),
        ) {
            Ok(gas_value) => match validated_rpc_quantity(&gas_value, "gas limit") {
                Ok(gas_limit) => Response::ok(json!({
                    "schema": "elastos.chain.gas_estimate/v1",
                    "network": network.id,
                    "from": from,
                    "to": to,
                    "value": value,
                    "data": data,
                    "gas_limit": gas_limit,
                })),
                Err(err) => Response::error("upstream_invalid_gas_limit", &err),
            },
            Err(response) => response,
        }
    }

    fn transaction_count(&self, network_id: &str, address: &str, block: Option<&str>) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_address(address) {
            return Response::error("invalid_address", &err);
        }
        let block = match normalize_block_tag(block) {
            Ok(block) => block,
            Err(err) => return Response::error("invalid_block", &err),
        };
        match self.evm_rpc(network, "eth_getTransactionCount", json!([address, block])) {
            Ok(count) => match validated_rpc_quantity(&count, "transaction count") {
                Ok(nonce) => Response::ok(json!({
                    "schema": "elastos.chain.transaction_count/v1",
                    "network": network.id,
                    "address": address,
                    "block": block,
                    "nonce": nonce,
                })),
                Err(err) => Response::error("upstream_invalid_transaction_count", &err),
            },
            Err(response) => response,
        }
    }

    fn gas_price(&self, network_id: &str) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        match self.evm_rpc(network, "eth_gasPrice", json!([])) {
            Ok(gas_price) => match validated_rpc_quantity(&gas_price, "gas price") {
                Ok(gas_price) => Response::ok(json!({
                    "schema": "elastos.chain.gas_price/v1",
                    "network": network.id,
                    "gas_price": gas_price,
                })),
                Err(err) => Response::error("upstream_invalid_gas_price", &err),
            },
            Err(response) => response,
        }
    }

    fn fee_history(
        &self,
        network_id: &str,
        block_count: &str,
        newest_block: &str,
        reward_percentiles: &[f64],
    ) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        if let Err(err) = validate_hex_quantity(block_count, "block count") {
            return Response::error("invalid_block_count", &err);
        }
        match parse_hex_u64(block_count) {
            Ok(count) if (1..=1024).contains(&count) => {}
            Ok(_) => {
                return Response::error(
                    "invalid_block_count",
                    "fee history block count must be between 1 and 1024",
                )
            }
            Err(err) => return Response::error("invalid_block_count", &err),
        }
        let newest_block = match validate_block_tag(newest_block, "newest block") {
            Ok(block) => block,
            Err(err) => return Response::error("invalid_newest_block", &err),
        };
        if reward_percentiles
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=100.0).contains(value))
        {
            return Response::error(
                "invalid_reward_percentiles",
                "reward percentiles must be finite values from 0 to 100",
            );
        }
        match self.evm_rpc(
            network,
            "eth_feeHistory",
            json!([block_count, newest_block, reward_percentiles]),
        ) {
            Ok(history) => Response::ok(json!({
                "schema": "elastos.chain.fee_history/v1",
                "network": network.id,
                "history": history,
            })),
            Err(response) => response,
        }
    }

    fn code(&self, network_id: &str, address: &str, block: Option<&str>) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_address(address) {
            return Response::error("invalid_address", &err);
        }
        let block = match normalize_block_tag(block) {
            Ok(block) => block,
            Err(err) => return Response::error("invalid_block", &err),
        };
        match self.evm_rpc(network, "eth_getCode", json!([address, block])) {
            Ok(code) => match code.as_str() {
                Some(code) => {
                    if let Err(err) = validate_hex(code, None, "contract code") {
                        return Response::error("upstream_invalid_code", &err);
                    }
                    Response::ok(json!({
                        "schema": "elastos.chain.code/v1",
                        "network": network.id,
                        "address": address,
                        "block": block,
                        "code": code,
                    }))
                }
                None => Response::error("upstream_invalid_code", "contract code must be hex"),
            },
            Err(response) => response,
        }
    }

    fn logs(&self, network_id: &str, filter: Value) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        let filter = match validate_evm_log_filter(filter) {
            Ok(filter) => filter,
            Err(err) => return Response::error("invalid_filter", &err),
        };
        match self.evm_rpc_logs(network, filter) {
            Ok(logs) => Response::ok(json!({
                "schema": "elastos.chain.logs/v1",
                "network": network.id,
                "logs": logs,
            })),
            Err(response) => response,
        }
    }

    fn sync_health(&self, network_id: &str) -> Response {
        let network = match self.network_for_status(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        match network.kind {
            ChainKind::EvmJsonRpc => self.evm_sync_health(network),
            ChainKind::BitcoinCoreRpc => self.bitcoin_sync_health(network),
            ChainKind::BitcoinRest => self.bitcoin_rest_sync_health(network),
            ChainKind::MainchainRest => self.mainchain_sync_health(network),
        }
    }

    fn evm_sync_health(&self, network: &ChainNetwork) -> Response {
        match self.evm_rpc(network, "eth_syncing", json!([])) {
            Ok(Value::Bool(false)) => Response::ok(json!({
                "network": network.public_view(),
                "synced": true,
                "syncing": false,
            })),
            Ok(Value::Object(sync)) => match evm_sync_object(sync) {
                Ok(sync) => Response::ok(json!({
                    "network": network.public_view(),
                    "synced": false,
                    "syncing": true,
                    "sync": sync,
                })),
                Err(err) => Response::error("upstream_invalid_sync", &err),
            },
            Ok(_) => Response::error(
                "upstream_invalid_sync",
                "eth_syncing must return false or a sync object",
            ),
            Err(response) => response,
        }
    }

    fn bitcoin_sync_health(&self, network: &ChainNetwork) -> Response {
        let info = match self.bitcoin_rpc(network, "getblockchaininfo", json!([])) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let blocks = info.get("blocks").and_then(Value::as_u64);
        let headers = info.get("headers").and_then(Value::as_u64);
        let initial_block_download = info
            .get("initialblockdownload")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let synced = !initial_block_download
            && blocks
                .zip(headers)
                .map(|(blocks, headers)| blocks >= headers)
                .unwrap_or(false);
        Response::ok(json!({
            "network": network.public_view(),
            "synced": synced,
            "syncing": !synced,
            "block_height": blocks,
            "headers": headers,
            "initial_block_download": initial_block_download,
            "verification_progress": info.get("verificationprogress").and_then(Value::as_f64),
        }))
    }

    fn bitcoin_rest_sync_health(&self, network: &ChainNetwork) -> Response {
        match self.bitcoin_rest_tip_height(network) {
            Ok(block_height) => Response::ok(json!({
                "network": network.public_view(),
                "synced": true,
                "syncing": false,
                "block_height": block_height,
                "backend": "remote_rest",
            })),
            Err(response) => response,
        }
    }

    fn mainchain_sync_health(&self, network: &ChainNetwork) -> Response {
        match self.mainchain_tip(network) {
            Ok(tip) => Response::ok(json!({
                "network": network.public_view(),
                "synced": true,
                "syncing": false,
                "block_height": tip.height,
                "best_block_hash": tip.hash,
                "backend": "remote_rest",
            })),
            Err(response) => response,
        }
    }

    fn transaction(&self, network_id: &str, hash: &str) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_hash(hash) {
            return Response::error("invalid_hash", &err);
        }
        match self.evm_rpc(network, "eth_getTransactionByHash", json!([hash])) {
            Ok(transaction) => Response::ok(json!({
                "network": network.id,
                "hash": hash,
                "transaction": transaction,
            })),
            Err(response) => response,
        }
    }

    fn receipt(&self, network_id: &str, hash: &str) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_hash(hash) {
            return Response::error("invalid_hash", &err);
        }
        match self.evm_rpc(network, "eth_getTransactionReceipt", json!([hash])) {
            Ok(receipt) => Response::ok(json!({
                "network": network.id,
                "hash": hash,
                "receipt": receipt,
            })),
            Err(response) => response,
        }
    }

    fn has_access_by_content_id(
        &self,
        network_id: &str,
        contract: &str,
        content_id: &str,
        subject: &str,
        right: &str,
    ) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_address(contract) {
            return Response::error("invalid_contract", &err);
        }
        if let Err(err) = validate_content_id(content_id) {
            return Response::error("invalid_content_id", &err);
        }
        if let Err(err) = validate_evm_address(subject) {
            return Response::error("invalid_subject", &err);
        }
        if let Err(err) = validate_right(right) {
            return Response::error("invalid_right", &err);
        }
        let method = match rights_method(network, "has_access_by_content_id", contract) {
            Ok(method) => method,
            Err(response) => return response,
        };
        let data = match method.abi {
            RightsMethodAbi::HasAccessByContentIdAddressBytes16 => {
                // Real Base ABI: `hasAccessByContentId(address holder, bytes16 contentId)`.
                // `right` is gateway-only (binary access on-chain), so it is not encoded.
                match encode_has_access_by_content_id_address_bytes16(
                    &method.selector,
                    subject,
                    content_id,
                ) {
                    Ok(data) => data,
                    Err(err) => return Response::error("invalid_rights_method", &err),
                }
            }
            RightsMethodAbi::HasAccessByContentIdStringAddressString => {
                match encode_has_access_by_content_id_call(
                    &method.selector,
                    content_id,
                    subject,
                    right,
                ) {
                    Ok(data) => data,
                    Err(err) => return Response::error("invalid_rights_method", &err),
                }
            }
        };
        let build = |has_access: bool| {
            Response::ok(json!({
                "network": network.id,
                "contract": method.contract.as_str(),
                "content_id": content_id,
                "subject": subject,
                "right": right,
                "has_access": has_access,
            }))
        };
        match self.evm_rpc(
            network,
            "eth_call",
            json!([{ "to": method.contract.as_str(), "data": data }, "latest"]),
        ) {
            Ok(result) => match decode_evm_bool(&result) {
                Ok(has_access) => build(has_access),
                Err(err) => Response::error("upstream_invalid_bool", &err),
            },
            // PC2 parity (`~/.pc2` `storage.ts`:
            // `gateway.hasAccessByContentId(holder, kid).catch(() => false)`): a CONTRACT
            // REVERT is a definitive "no access" for this contentId (the content is not
            // registered, or the holder has no access record) — NOT an outage. Map it to
            // `has_access: false` so the rights gate fails CLOSED cleanly (a 403 denial)
            // instead of surfacing a 503. Genuine transport/RPC failures still propagate, so
            // an RPC outage can never masquerade as a certain denial.
            Err(response) if is_contract_revert(&response) => build(false),
            Err(response) => response,
        }
    }

    fn proof(&self, network_id: &str, proof_kind: ChainProofKind, subject: &str) -> Response {
        if let Err(err) = validate_subject(subject) {
            return Response::error("invalid_subject", &err);
        }
        let evidence = match proof_kind {
            ChainProofKind::Status => match self.status(network_id) {
                Response::Ok { data: Some(data) } => data,
                Response::Error { code, message } => return Response::Error { code, message },
                Response::Ok { data: None } => {
                    return Response::error("missing_evidence", "status proof missing evidence")
                }
            },
            ChainProofKind::SyncHealth => match self.sync_health(network_id) {
                Response::Ok { data: Some(data) } => data,
                Response::Error { code, message } => return Response::Error { code, message },
                Response::Ok { data: None } => {
                    return Response::error("missing_evidence", "sync proof missing evidence")
                }
            },
        };
        Response::ok(json!({
            "schema": "elastos.chain.proof/v1",
            "network": network_id,
            "proof_kind": proof_kind,
            "subject": subject,
            "evidence_hash": value_hash(&evidence),
            "created_at": now_ts(),
        }))
    }

    fn erc1271_is_valid_signature(
        &self,
        network_id: &str,
        contract: &str,
        message_hash: &str,
        signature: &str,
    ) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_address(contract) {
            return Response::error("invalid_contract", &err);
        }
        let message_hash_bytes = match decode_hex(message_hash, Some(32), "message_hash") {
            Ok(bytes) => bytes,
            Err(err) => return Response::error("invalid_message_hash", &err),
        };
        let signature_bytes = match decode_hex(signature, None, "signature") {
            Ok(bytes) if !bytes.is_empty() && bytes.len() <= 4096 => bytes,
            Ok(_) => return Response::error("invalid_signature", "signature must be 1-4096 bytes"),
            Err(err) => return Response::error("invalid_signature", &err),
        };
        let data = encode_erc1271_is_valid_signature_call(&message_hash_bytes, &signature_bytes);
        let result = match self.evm_rpc(
            network,
            "eth_call",
            json!([{ "to": contract, "data": data }, "latest"]),
        ) {
            Ok(result) => result,
            Err(response) => return response,
        };
        let magic_value = match decode_erc1271_magic_value(&result) {
            Ok(value) => value,
            Err(err) => return Response::error("upstream_invalid_erc1271", &err),
        };
        Response::ok(json!({
            "schema": "elastos.chain.erc1271_proof/v1",
            "network": network.public_view(),
            "chain_id": network.chain_id,
            "contract": normalize_evm_address(contract),
            "message_hash": message_hash,
            "signature_hash": bytes_hash(&signature_bytes),
            "valid": magic_value == "0x1626ba7e",
            "magic_value": magic_value,
            "checked_at": now_ts(),
        }))
    }

    fn prepare_transaction(
        &self,
        network_id: &str,
        from: &str,
        to: &str,
        value: &str,
        data: Option<&str>,
    ) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_address(from) {
            return Response::error("invalid_from", &err);
        }
        if let Err(err) = validate_evm_address(to) {
            return Response::error("invalid_to", &err);
        }
        if let Err(err) = validate_hex_quantity(value, "value") {
            return Response::error("invalid_value", &err);
        }
        let data = data.unwrap_or("0x");
        if let Err(err) = validate_hex(data, None, "transaction data") {
            return Response::error("invalid_data", &err);
        }
        if data.len() > 256 * 1024 {
            return Response::error("invalid_data", "transaction data is too large");
        }
        let Some(chain_id) = network.chain_id else {
            return Response::error("invalid_network", "EVM network missing chain_id");
        };
        let nonce = match self.evm_rpc(network, "eth_getTransactionCount", json!([from, "pending"]))
        {
            Ok(value) => match validated_rpc_quantity(&value, "transaction nonce") {
                Ok(value) => value,
                Err(err) => return Response::error("upstream_invalid_nonce", &err),
            },
            Err(response) => return response,
        };
        let gas_price = match self.evm_rpc(network, "eth_gasPrice", json!([])) {
            Ok(value) => match validated_rpc_quantity(&value, "gas price") {
                Ok(value) => value,
                Err(err) => return Response::error("upstream_invalid_gas_price", &err),
            },
            Err(response) => return response,
        };
        let gas_limit = match self.evm_rpc(
            network,
            "eth_estimateGas",
            json!([{ "from": from, "to": to, "value": value, "data": data }]),
        ) {
            Ok(value) => match validated_rpc_quantity(&value, "gas limit") {
                Ok(value) => value,
                Err(err) => return Response::error("upstream_invalid_gas_limit", &err),
            },
            Err(response) => return response,
        };
        Response::ok(json!({
            "schema": "elastos.chain.unsigned_transaction_intent/v1",
            "transaction_type": "eip155_legacy",
            "network": network.public_view(),
            "from": from,
            "to": to,
            "value": value,
            "data": data,
            "chain_id": chain_id,
            "nonce": nonce,
            "gas_price": gas_price,
            "gas_limit": gas_limit,
            "requires_wallet_approval": true,
            "wallet_intent": "transaction_intent",
        }))
    }

    /// Assemble the dDRM content-mint calldata from a structured `MintAssembly`. PURE:
    /// no RPC and no keys — it ABI-encodes the PC2 `mint(string,uint16,bytes,bytes)` call
    /// (opRawData leading with the `bytes16` contentId; sellRawData = copies/price/token)
    /// and returns the `{ to, data, value }` an external signer signs and
    /// `broadcast_transaction` sends. Fail-closed on a malformed mint.
    fn assemble_mint(&self, mint: MintAssembly) -> Response {
        if let Err(err) = validate_evm_address(&mint.to) {
            return Response::error("invalid_to", &err);
        }
        if mint.token_uri.trim().is_empty() {
            return Response::error("invalid_mint", "token_uri must not be empty");
        }
        let value = mint.value_wei.as_deref().unwrap_or("0x0");
        if let Err(err) = validate_hex_quantity(value, "value") {
            return Response::error("invalid_value", &err);
        }

        // FREE: opRawData = abi.encode(bytes16); sellRawData = empty. PAID: full op/sell.
        let (op_raw, sell_raw) = if mint.op_type_code == 0 {
            if mint.op_raw.is_some() || mint.sell.is_some() {
                return Response::error(
                    "invalid_mint",
                    "a free mint (op_type_code 0) must not carry op_raw/sell terms",
                );
            }
            match encode_op_raw_free(&mint.content_id) {
                Ok(op) => (op, Vec::new()),
                Err(err) => return Response::error("invalid_mint", &err),
            }
        } else {
            let (Some(op_raw), Some(sell)) = (mint.op_raw.as_ref(), mint.sell.as_ref()) else {
                return Response::error(
                    "invalid_mint",
                    "a paid mint requires both op_raw and sell terms",
                );
            };
            // BUY_AND_RESELL (2) carries the trailing uint16 resellerCut; BUY_ONCE (1)
            // must not (it would shift the ABI layout).
            let reseller_cut = match (mint.op_type_code, op_raw.reseller_cut) {
                (2, Some(cut)) => Some(cut),
                (2, None) => {
                    return Response::error(
                        "invalid_mint",
                        "buy_and_resell (op_type_code 2) requires op_raw.reseller_cut",
                    )
                }
                (1, None) => None,
                (1, Some(_)) => {
                    return Response::error(
                        "invalid_mint",
                        "buy_once (op_type_code 1) must not carry op_raw.reseller_cut",
                    )
                }
                _ => return Response::error("invalid_mint", "unsupported op_type_code"),
            };
            let op = match encode_op_raw_paid(
                &mint.content_id,
                &op_raw.metadata_uri,
                &op_raw.addresses,
                &op_raw.role_types,
                &op_raw.amounts,
                reseller_cut,
            ) {
                Ok(op) => op,
                Err(err) => return Response::error("invalid_mint", &err),
            };
            let sell = match encode_sell_raw_data(&sell.copies, &sell.price_wei, &sell.pay_token) {
                Ok(sell) => sell,
                Err(err) => return Response::error("invalid_mint", &err),
            };
            (op, sell)
        };

        let data = match encode_mint_calldata(
            &mint.selector,
            &mint.token_uri,
            mint.op_type_code,
            &op_raw,
            &sell_raw,
        ) {
            Ok(data) => data,
            Err(err) => return Response::error("invalid_mint", &err),
        };

        Response::ok(json!({
            "schema": "elastos.chain.mint_assembly/v1",
            "function": MINT_SIGNATURE,
            "to": mint.to,
            "data": data,
            "value": value,
            "op_type_code": mint.op_type_code,
            "content_id": mint.content_id,
            // Pure assembly: never signed, never broadcast here.
            "signed": false,
            "next_required_providers": ["wallet-provider", "chain-provider"],
        }))
    }

    /// Max `eth_getLogs` window span (env `ELASTOS_CHANNEL_MAX_LOG_RANGE`, else 10000).
    fn max_log_range() -> u64 {
        std::env::var("ELASTOS_CHANNEL_MAX_LOG_RANGE")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_MAX_LOG_RANGE)
    }

    /// Backfill windows scanned per call (env `ELASTOS_CHANNEL_BACKFILL_WINDOWS`, else 24).
    fn backfill_windows_per_call() -> u64 {
        std::env::var("ELASTOS_CHANNEL_BACKFILL_WINDOWS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_BACKFILL_WINDOWS_PER_CALL)
    }

    /// Latest block height for an EVM network.
    fn evm_latest_block(&self, network: &ChainNetwork) -> Result<u64, Response> {
        let value = self.evm_rpc(network, "eth_blockNumber", json!([]))?;
        let hex = value
            .as_str()
            .ok_or_else(|| Response::error("upstream_invalid_block", "blockNumber not a string"))?;
        parse_hex_u64(hex).map_err(|err| Response::error("upstream_invalid_block", &err))
    }

    /// Does an RPC error look like a "block range too large" cap (vs a real failure)? Endpoints
    /// phrase it differently — "exceed maximum block range", "ranges over 10000 blocks are not
    /// supported", "eth_getLogs is limited to a 10,000 range", "limited to 0 - 50 blocks". We
    /// match on the shared shape so the scanner can split-and-retry instead of failing closed.
    fn is_range_limit_error(response: &Response) -> bool {
        let Response::Error { code, message } = response else {
            return false;
        };
        let msg = message.to_ascii_lowercase();
        // Some endpoints reject an over-cap range at the HTTP layer rather than as a JSON-RPC
        // error: `mainnet.base.org` → HTTP 413, `drpc` free tier → HTTP 400. Treat those as
        // range signals too so the scanner splits instead of failing the whole discovery.
        if code == "upstream_http_error" {
            return msg.contains("413") || msg.contains("400");
        }
        if code != "upstream_rpc_error" {
            return false;
        }
        msg.contains("block range")
            || msg.contains("blocks range")
            || msg.contains("blocks are not supported")
            || msg.contains("limited to")
            || msg.contains("exceed maximum")
    }

    /// Adaptively scan an inclusive `[from, to]` block window of the factory's `ChannelCreated`
    /// logs for one creator, folding results into `entry`. READ-ONLY (`eth_getLogs`, range-capable
    /// pool). On a "range too large" signal (JSON code OR HTTP 400/413) it halves the window and
    /// scans both halves, so a stricter endpoint still completes. A malformed/foreign log is
    /// skipped; a non-range RPC/transport failure propagates (caller fails closed).
    fn scan_channel_window(
        &self,
        network: &ChainNetwork,
        factory: &str,
        creator_topic: &str,
        from: u64,
        to: u64,
        entry: &mut ChannelIndexEntry,
    ) -> Result<usize, Response> {
        // ChannelCreated(uint8 indexed channelType, uint8 indexed scope, address indexed
        // creator, address channel, address factoryAddr): topic0 + creator (4th topic).
        let filter = json!({
            "address": factory,
            "fromBlock": format!("0x{from:x}"),
            "toBlock": format!("0x{to:x}"),
            "topics": [CHANNEL_CREATED_TOPIC0, Value::Null, Value::Null, creator_topic],
        });
        let logs = match self.evm_rpc_logs(network, filter) {
            Ok(logs) => logs,
            Err(response) => {
                // Split-and-retry only when the span is still divisible and the endpoint
                // complained about range size; otherwise fail closed.
                if Self::is_range_limit_error(&response) && to.saturating_sub(from) >= MIN_LOG_RANGE
                {
                    let mid = from + (to - from) / 2;
                    let lower = self.scan_channel_window(
                        network,
                        factory,
                        creator_topic,
                        from,
                        mid,
                        entry,
                    )?;
                    let upper = self.scan_channel_window(
                        network,
                        factory,
                        creator_topic,
                        mid + 1,
                        to,
                        entry,
                    )?;
                    return Ok(lower + upper);
                }
                return Err(response);
            }
        };
        let entries = logs.as_array().ok_or_else(|| {
            Response::error(
                "upstream_invalid_logs",
                "eth_getLogs result was not an array",
            )
        })?;
        let mut found = 0usize;
        for log in entries {
            let Ok(decoded) = decode_channel_log(log) else {
                continue;
            };
            let Some(address) = decoded.get("address").and_then(Value::as_str) else {
                continue;
            };
            let block_number = decoded
                .get("block_number")
                .and_then(Value::as_u64)
                .unwrap_or(from);
            let ct = decoded
                .get("channel_type")
                .and_then(Value::as_u64)
                .map(|v| v as u8);
            let scope = decoded
                .get("scope")
                .and_then(Value::as_u64)
                .map(|v| v as u8);
            entry.upsert(address, block_number, ct, scope);
            found += 1;
        }
        Ok(found)
    }

    /// Resolve the real ledger `tokenId` for a `bytes16` KID by scanning the channel/ledger's
    /// `AssetCreated` logs (the only mint event that emits on Base) and binding the KID via each
    /// candidate's mint calldata (`opRawData`) — newest-first, split-and-retry. READ-ONLY
    /// (`eth_getLogs` + `eth_getTransactionByHash`); no keys. The Phase-1 buy binds THIS, never a hash
    /// of the content id. Fails closed if no `AssetCreated` on the ledger binds the KID.
    fn resolve_token_id(
        &mut self,
        network_id: &str,
        ledger: &str,
        content_id: &str,
        from_block: Option<&str>,
    ) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network.clone(),
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_address(ledger) {
            return Response::error("invalid_ledger", &err);
        }
        let want = match normalize_content_id_bytes16(content_id) {
            Some(kid) => kid,
            None => return Response::error("invalid_content_id", "content_id is not a bytes16 KID"),
        };
        let channel_topic = match address_topic(ledger) {
            Ok(topic) => topic,
            Err(err) => return Response::error("invalid_ledger", &err),
        };
        let deploy_block = match from_block {
            Some(value) => match parse_hex_u64(value.trim()) {
                Ok(value) => value,
                Err(err) => return Response::error("invalid_from_block", &err),
            },
            None => DEFAULT_CHANNEL_FROM_BLOCK,
        };
        let latest = match self.evm_latest_block(&network) {
            Ok(latest) => latest,
            Err(response) => return response,
        };
        let window = Self::max_log_range().max(1);
        // MKT-1 fail-closed GLOBAL uniqueness: accumulate EVERY distinct (operative, tokenId) that
        // binds the KID across the WHOLE channel range — we do NOT return on the first window — and
        // bind ONLY if exactly one exists. A hostile co-channel mint that re-uses the victim's public
        // KID (in the same OR a newer window) produces a second distinct binder, so the resolve fails
        // closed rather than mis-charge the buyer. The channel-topic filter keeps the scan bounded to
        // one channel's mints; the early-exit below caps the griefing/ambiguous path.
        let mut found: std::collections::BTreeMap<(String, String), u64> =
            std::collections::BTreeMap::new();
        let mut to = latest;
        loop {
            let from = to.saturating_sub(window - 1).max(deploy_block);
            match self.scan_asset_created_window_for_kid(&network, &channel_topic, &want, from, to) {
                Ok(hits) => {
                    for (operative, token_id, block) in hits {
                        found.entry((operative, token_id)).or_insert(block);
                    }
                    // Once two DISTINCT binders exist, more scanning cannot make the binding unique —
                    // fail closed now (also bounds RPC on the ambiguous/griefing path).
                    if found.len() >= 2 {
                        break;
                    }
                }
                Err(response) => return response,
            }
            if from <= deploy_block {
                break;
            }
            to = from.saturating_sub(1);
        }
        match found.len() {
            1 => {
                let ((operative, token_id), block) = found.into_iter().next().unwrap();
                Response::ok(json!({
                    "content_id": format!("0x{want}"),
                    "token_id": token_id,
                    "operative": operative,
                    "ledger": ledger,
                    "block": block,
                    "chain": network.id,
                }))
            }
            0 => Response::error(
                "token_id_not_found",
                &format!(
                    "no AssetCreated on ledger {ledger} whose mint binds KID 0x{want} in [{deploy_block}, {latest}]"
                ),
            ),
            _ => Response::error(
                "ambiguous_kid_binding",
                &format!(
                    "KID 0x{want} on ledger {ledger} binds >1 distinct (operative, tokenId) — refusing to bind a possibly-hostile token (buy blocked, fail-closed)"
                ),
            ),
        }
    }

    /// Scan one window of the channel's `AssetCreated` logs (any creator), fetch each candidate mint
    /// tx's calldata, and return the `(operative, token_id, block)` whose mint binds the KID. Split-and-
    /// retry on a range-limit error (mirrors `fetch_asset_created_logs`); the pure bind is
    /// `collect_kid_bindings` (union of both bind methods; the caller requires global uniqueness).
    fn scan_asset_created_window_for_kid(
        &self,
        network: &ChainNetwork,
        channel_topic: &str,
        want_kid: &str,
        from: u64,
        to: u64,
    ) -> Result<Vec<(String, String, u64)>, Response> {
        let filter = json!({
            "fromBlock": format!("0x{from:x}"),
            "toBlock": format!("0x{to:x}"),
            "topics": [ASSET_CREATED_TOPIC0, Value::Null, channel_topic],
        });
        let logs = match self.evm_rpc_logs(network, filter) {
            Ok(logs) => logs,
            Err(response) => {
                if Self::is_range_limit_error(&response) && to.saturating_sub(from) >= MIN_LOG_RANGE {
                    let mid = from + (to - from) / 2;
                    // Concatenate BOTH halves — MKT-1 global uniqueness needs every binder, so a
                    // split-retry must never drop the lower half after a hit in the upper half.
                    let mut hits = self.scan_asset_created_window_for_kid(
                        network, channel_topic, want_kid, mid + 1, to,
                    )?;
                    hits.extend(self.scan_asset_created_window_for_kid(
                        network, channel_topic, want_kid, from, mid,
                    )?);
                    return Ok(hits);
                }
                return Err(response);
            }
        };
        let entries = logs.as_array().ok_or_else(|| {
            Response::error("upstream_invalid_logs", "eth_getLogs result was not an array")
        })?;
        let mut decoded: Vec<(String, String, u64, u64, String)> = Vec::new();
        for log in entries {
            let Some((operative, token_id, block, log_index)) = decode_asset_created_log(log) else {
                continue;
            };
            let Some(tx_hash) = log.get("transactionHash").and_then(Value::as_str) else {
                continue;
            };
            decoded.push((operative, token_id, block, log_index, tx_hash.to_string()));
        }
        decoded.sort_by(|a, b| (b.2, b.3).cmp(&(a.2, a.3))); // newest-first
        // Fetch each candidate's mint calldata (live), then bind the KID purely.
        let mut inputs = std::collections::HashMap::new();
        for (_, _, _, _, tx_hash) in &decoded {
            if inputs.contains_key(tx_hash) {
                continue;
            }
            match self.tx_input(network, tx_hash)? {
                Some(input) => {
                    inputs.insert(tx_hash.clone(), input);
                }
                // MKT-1 fail-closed-by-omission hardening: if a candidate mint's calldata cannot be
                // fetched we CANNOT evaluate whether it binds the KID — silently skipping it could
                // drop the legit binder and leave a hostile singleton (mis-bind). Abort the whole
                // resolve rather than decide on a partial candidate set.
                None => {
                    return Err(Response::error(
                        "candidate_input_unavailable",
                        &format!(
                            "mint calldata for candidate tx {tx_hash} is unavailable; refusing to resolve on a partial candidate set (fail-closed)"
                        ),
                    ));
                }
            }
        }
        // Return ALL distinct binders in this window (with a representative block) — the caller folds
        // them across windows and requires GLOBAL uniqueness (MKT-1 fail-closed).
        let bindings = collect_kid_bindings(&decoded, &inputs, want_kid);
        let mut hits = Vec::with_capacity(bindings.len());
        for (operative, token_id) in bindings {
            let block = decoded
                .iter()
                .find(|entry| entry.1 == token_id && entry.0 == operative)
                .map(|entry| entry.2)
                .unwrap_or(from);
            hits.push((operative, token_id, block));
        }
        Ok(hits)
    }

    /// Discover a creator's dDRM channels via a PERSISTED, RESUMABLE factory scan. Mirrors
    /// PC2's `ContentIndexerService` cursor model (forward `head` + backfill `floor`), adapted
    /// to the runtime's synchronous provider model: each call scans new blocks since `head`
    /// (cheap) and lowers `floor` toward the deploy block by a bounded budget of windows
    /// (newest-first, early-surfacing). The index is an untrusted cache (#5) — the chain stays
    /// canonical (#10) and a selected channel is re-confirmed on-chain before any mint (#11).
    fn list_channels(
        &mut self,
        network_id: &str,
        factory: Option<&str>,
        creator: &str,
        from_block: Option<&str>,
    ) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network.clone(),
            Err(response) => return response,
        };
        let factory = factory.unwrap_or(DEFAULT_CHANNEL_FACTORY).to_string();
        if let Err(err) = validate_evm_address(&factory) {
            return Response::error("invalid_factory", &err);
        }
        if let Err(err) = validate_evm_address(creator) {
            return Response::error("invalid_creator", &err);
        }
        // Per-request override of the backfill lower bound (else the pinned deploy block).
        let deploy_block = match from_block {
            Some(value) => match parse_hex_u64(value.trim()) {
                Ok(value) => value,
                Err(err) => return Response::error("invalid_from_block", &err),
            },
            None => DEFAULT_CHANNEL_FROM_BLOCK,
        };
        let creator_topic = match address_topic(creator) {
            Ok(topic) => topic,
            Err(err) => return Response::error("invalid_creator", &err),
        };
        let latest = match self.evm_latest_block(&network) {
            Ok(latest) => latest,
            Err(response) => return response,
        };

        let key = channel_index_key(&network.id, &factory, creator);
        let mut entry = self
            .channel_index
            .entries
            .get(&key)
            .cloned()
            .unwrap_or_else(|| {
                // Fresh cursor: nothing scanned yet, both ends pinned at the chain head so the
                // first forward pass is a no-op and backfill starts walking down from the tip.
                ChannelIndexEntry {
                    deploy_block,
                    floor: latest.saturating_add(1),
                    head: latest,
                    complete: false,
                    channels: Vec::new(),
                    updated_at: now_ts(),
                }
            });
        // A per-request deploy override re-opens backfill toward the new (lower) bound.
        if deploy_block < entry.deploy_block {
            entry.deploy_block = deploy_block;
            entry.complete = false;
        }

        let max_range = Self::max_log_range();
        let window = max_range.max(1);

        // 1) Forward/incremental: scan new blocks since `head` (cheap; keeps the list fresh).
        if latest > entry.head {
            let mut from = entry.head.saturating_add(1);
            while from <= latest {
                let to = (from.saturating_add(window - 1)).min(latest);
                if let Err(response) = self.scan_channel_window(
                    &network,
                    &factory,
                    &creator_topic,
                    from,
                    to,
                    &mut entry,
                ) {
                    return response;
                }
                if to == latest {
                    break;
                }
                from = to.saturating_add(1);
            }
            entry.head = latest;
        }

        // 2) Backfill (resumable, newest-first): lower `floor` toward `deploy_block` by a
        //    bounded budget so a single synchronous call stays responsive. Early-surface:
        //    stop this call as soon as we've found channel(s), persisting progress so a later
        //    call resumes downward — recent channels (the common case) appear in the first call.
        if !entry.complete && entry.floor > entry.deploy_block {
            let budget = Self::backfill_windows_per_call();
            let mut scanned = 0u64;
            let pre_existing = entry.channels.len();
            while scanned < budget && entry.floor > entry.deploy_block {
                let to = entry.floor.saturating_sub(1);
                let from = to.saturating_sub(window - 1).max(entry.deploy_block);
                let found = match self.scan_channel_window(
                    &network,
                    &factory,
                    &creator_topic,
                    from,
                    to,
                    &mut entry,
                ) {
                    Ok(found) => found,
                    Err(response) => return response,
                };
                entry.floor = from;
                scanned += 1;
                if from <= entry.deploy_block {
                    entry.complete = true;
                    break;
                }
                // Early-surface: once this call has discovered new channels, return so the UI
                // is responsive. The lowered `floor` is persisted; coverage resumes next call.
                if found > 0 && entry.channels.len() > pre_existing {
                    break;
                }
            }
        }

        entry.updated_at = now_ts();
        let ordered = entry.channels_newest_first();
        let channels: Vec<Value> = ordered
            .iter()
            .map(|c| {
                json!({
                    "address": c.address,
                    "channel_type": c.channel_type,
                    "scope": c.scope,
                    "block_number": c.block_number,
                })
            })
            .collect();
        let scanned_floor = entry.floor;
        let complete = entry.complete;
        let head = entry.head;
        let deploy = entry.deploy_block;
        // Persist the advanced cursor. A write failure only costs a rescan next time, so it
        // must not fail the (valid) read — surface it as a soft warning instead.
        self.channel_index.entries.insert(key, entry);
        let persist_warning =
            write_channel_index_file(&self.channel_index_path, &self.channel_index).err();

        Response::ok(json!({
            "schema": "elastos.chain.channels/v1",
            "network": network.id,
            "factory": factory,
            "creator": normalize_evm_address(creator),
            "channels": channels,
            // Cursor state so the UI can show "indexing… N%" and re-poll until complete.
            "indexing": !complete,
            "scanned_floor": scanned_floor,
            "scanned_head": head,
            "deploy_block": deploy,
            "latest_block": latest,
            "index_warning": persist_warning,
        }))
    }

    /// Assemble the `createChannel(uint8,uint8,string,string,bytes)` calldata (PURE: no RPC,
    /// no keys) — the `{ to, data, value }` an external signer sends to deploy a channel.
    fn assemble_create_channel(&self, channel: CreateChannelAssembly) -> Response {
        if let Err(err) = validate_evm_address(&channel.factory) {
            return Response::error("invalid_factory", &err);
        }
        if channel.name.trim().is_empty() {
            return Response::error("invalid_channel", "channel name is required");
        }
        if channel.token_uri.trim().is_empty() {
            return Response::error("invalid_channel", "channel token URI is required");
        }
        let data = match channel.data_hex.as_deref() {
            Some(hex) => match decode_hex(hex, None, "channel data") {
                Ok(bytes) => bytes,
                Err(err) => return Response::error("invalid_channel", &err),
            },
            None => Vec::new(),
        };
        let value = match channel.value_wei.as_deref() {
            Some(value) => {
                if let Err(err) = validate_hex_quantity(value, "value") {
                    return Response::error("invalid_channel", &err);
                }
                value.to_string()
            }
            None => "0x0".to_string(),
        };
        let calldata = match encode_create_channel_calldata(
            &channel.selector,
            channel.channel_type,
            channel.scope,
            &channel.name,
            &channel.token_uri,
            &data,
        ) {
            Ok(data) => data,
            Err(err) => return Response::error("invalid_channel", &err),
        };
        Response::ok(json!({
            "schema": "elastos.chain.create_channel_assembly/v1",
            "function": CREATE_CHANNEL_SIGNATURE,
            "to": channel.factory,
            "data": calldata,
            "value": value,
            // Pure assembly: never signed, never broadcast here.
            "signed": false,
            "next_required_providers": ["wallet-provider", "chain-provider"],
        }))
    }

    /// Assemble the post-mint trade-enabling approval (PC2's 2nd mint tx). Confirmation-gated:
    /// the operative contract is read from the just-minted asset's `AssetCreated` log (which a
    /// chain only emits on a SUCCESSFUL mint), so a missing log means "mint not confirmed yet"
    /// and we fail closed (#11). Idempotent: if the gateway is already an approved operator we
    /// return `already_approved` rather than queueing a needless second signature.
    fn assemble_trade_approval(
        &self,
        network_id: &str,
        channel: &str,
        creator: &str,
        content_id: Option<&str>,
        tx_hash: Option<&str>,
    ) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network.clone(),
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_address(channel) {
            return Response::error("invalid_channel", &err);
        }
        if let Err(err) = validate_evm_address(creator) {
            return Response::error("invalid_creator", &err);
        }
        let creator_topic = match address_topic(creator) {
            Ok(topic) => topic,
            Err(err) => return Response::error("invalid_creator", &err),
        };
        let channel_topic = match address_topic(channel) {
            Ok(topic) => topic,
            Err(err) => return Response::error("invalid_channel", &err),
        };
        // When a content id is supplied, pin to it (fail-closed if it is not a clean bytes16 KID).
        let want_content_id =
            match content_id {
                Some(cid) => match normalize_content_id_bytes16(cid) {
                    Some(norm) => Some(norm),
                    None => return Response::error(
                        "invalid_content_id",
                        "content_id must be a 16-byte (bytes16) hex KID to pin the trade approval",
                    ),
                },
                None => None,
            };
        // FAST PATH (PC2 `tx.wait()` parity): if the caller knows the broadcast mint tx hash, the
        // asset's `AssetCreated` event is in THAT transaction's own receipt — resolve the operative
        // in one `eth_getTransactionReceipt` instead of a wide `eth_getLogs` scan (which public RPCs
        // rate-limit/range-cap, the cause of Step 2 never unlocking). A pending/mismatched receipt
        // falls through to the scan below, so nothing regresses when the hash is absent/not-yet-mined.
        if let Some(hash) = tx_hash.map(str::trim).filter(|h| !h.is_empty()) {
            match self.resolve_mint_from_receipt(
                &network,
                hash,
                &creator_topic,
                &channel_topic,
                want_content_id.as_deref(),
            ) {
                Ok(Some((operative, token_id))) => {
                    return self
                        .finish_trade_approval(&network, channel, creator, operative, token_id)
                }
                Ok(None) => { /* receipt pending / no match → fall back to the scan */ }
                Err(response) => return response,
            }
        }
        let latest = match self.evm_latest_block(&network) {
            Ok(latest) => latest,
            Err(response) => return response,
        };
        // The asset's `AssetCreated` log may sit well behind the chain tip (a mint minutes OR
        // days ago — Base produces ~30k blocks/day, so a single 10k window only covers a few
        // hours). Scan newest-first in `max_log_range` windows down toward the channel-factory
        // deploy block. With a content-id PIN we accept only the asset whose mint tx embeds THAT
        // KID (so an earlier, already-approved asset in the same channel can never be mistaken
        // for the just-minted one); without a pin we early-stop at the channel's newest mint.
        let window = Self::max_log_range().max(1);
        let floor = DEFAULT_CHANNEL_FROM_BLOCK;
        let mut to = latest;
        let mut budget = TRADE_APPROVAL_SCAN_WINDOWS;
        let found = loop {
            if budget == 0 || to < floor {
                break None;
            }
            let from = to.saturating_sub(window - 1).max(floor);
            let hit = match &want_content_id {
                Some(cid) => self.scan_asset_created_for_content_id(
                    &network,
                    &creator_topic,
                    &channel_topic,
                    cid,
                    from,
                    to,
                ),
                None => self
                    .scan_latest_asset_created(&network, &creator_topic, &channel_topic, from, to)
                    .map(|opt| opt.map(|(op, tid, _, _)| (op, tid))),
            };
            match hit {
                Ok(Some(hit)) => break Some(hit),
                Ok(None) => {}
                Err(response) => return response,
            }
            if from <= floor {
                break None;
            }
            to = from.saturating_sub(1);
            budget -= 1;
        };
        let (operative, token_id) = match found {
            Some(hit) => hit,
            None => {
                let detail = if want_content_id.is_some() {
                    "this asset's mint is not confirmed on-chain yet — wait for it to mine and retry"
                } else {
                    "no confirmed mint found for this wallet in this channel — if you just minted, wait for it to confirm on-chain and retry"
                };
                return Response::error("mint_not_confirmed", detail);
            }
        };
        self.finish_trade_approval(&network, channel, creator, operative, token_id)
    }

    /// Given the resolved `(operative, token_id)` for a confirmed mint, read the channel authority,
    /// short-circuit if the gateway is already approved, and otherwise assemble the unsigned
    /// `setApprovalForAll(gateway, true)` calldata. Shared by the receipt fast-path and the
    /// log-scan path so both return the identical assembly shape.
    fn finish_trade_approval(
        &self,
        network: &ChainNetwork,
        channel: &str,
        creator: &str,
        operative: String,
        token_id: String,
    ) -> Response {
        // The channel's `authority()` is the gateway that needs operator rights; fall back to
        // the configured default if the read misses (PC2 does the same — app.js:1674).
        let gateway = self
            .read_authority(network, channel)
            .unwrap_or_else(|| DEFAULT_AUTHORITY_GATEWAY.to_string());

        // Idempotent: already approved => no second signature needed.
        match self.read_is_approved_for_all(network, &operative, creator, &gateway) {
            Ok(true) => {
                return Response::ok(json!({
                    "schema": "elastos.chain.trade_approval_assembly/v1",
                    "already_approved": true,
                    "operative": operative,
                    "gateway": gateway,
                    "token_id": token_id,
                }))
            }
            Ok(false) => {}
            Err(response) => return response,
        }

        let data = match encode_set_approval_for_all_calldata(
            SET_APPROVAL_FOR_ALL_SELECTOR,
            &gateway,
            true,
        ) {
            Ok(data) => data,
            Err(err) => return Response::error("invalid_approval", &err),
        };
        Response::ok(json!({
            "schema": "elastos.chain.trade_approval_assembly/v1",
            "function": SET_APPROVAL_FOR_ALL_SIGNATURE,
            "already_approved": false,
            "to": operative,
            "data": data,
            "value": "0x0",
            "operative": operative,
            "gateway": gateway,
            "token_id": token_id,
            // Pure assembly: never signed, never broadcast here.
            "signed": false,
            "next_required_providers": ["wallet-provider", "chain-provider"],
        }))
    }

    /// FAST PATH resolver: read the mint's `(operative, token_id)` straight from its TRANSACTION
    /// RECEIPT (`eth_getTransactionReceipt`) instead of scanning `eth_getLogs` windows. The mint's
    /// `AssetCreated` event is in this tx's OWN receipt logs, so one cheap call confirms it — which
    /// also works on rate-limited public RPCs where wide scans fail. Returns:
    ///   `Ok(Some((operative, token_id)))` — receipt mined + succeeded + a matching `AssetCreated`,
    ///   `Ok(None)`                        — receipt not available yet (pending) OR no matching log
    ///                                       (caller falls back to the scan),
    ///   `Err(Response)`                   — RPC error, or the mint transaction REVERTED.
    fn resolve_mint_from_receipt(
        &self,
        network: &ChainNetwork,
        tx_hash: &str,
        creator_topic: &str,
        channel_topic: &str,
        want_content_id: Option<&str>,
    ) -> Result<Option<(String, String)>, Response> {
        // A malformed hash is not fatal — just decline the fast path so the scan still runs.
        if validate_evm_hash(tx_hash).is_err() {
            return Ok(None);
        }
        let receipt = self.evm_rpc(network, "eth_getTransactionReceipt", json!([tx_hash]))?;
        // `null` receipt ⇒ the tx is not mined yet (still pending) — not confirmed, not an error.
        if receipt.is_null() {
            return Ok(None);
        }
        // A reverted mint has `status == 0x0`: the asset was NOT created — fail closed.
        if let Some(status) = receipt.get("status").and_then(Value::as_str) {
            if parse_hex_u64(status.trim()).unwrap_or(1) == 0 {
                return Err(Response::error(
                    "mint_reverted",
                    "the mint transaction reverted on-chain",
                ));
            }
        }
        let Some(logs) = receipt.get("logs").and_then(Value::as_array) else {
            return Ok(None);
        };
        let Some((operative, token_id)) =
            newest_asset_created_in_logs(logs, creator_topic, channel_topic)
        else {
            return Ok(None);
        };
        // Optional content-id binding (defense in depth): the receipt is already the owner's own
        // mint, but if a KID is pinned, confirm the tx calldata embeds it before trusting the hash.
        if let Some(want) = want_content_id {
            if let Some(input) = self.tx_input(network, tx_hash)? {
                let bound = decode_mint_content_id(&input)
                    .and_then(|c| normalize_content_id_bytes16(&c))
                    .as_deref()
                    == Some(want)
                    || mint_input_binds_content_id(&input, want);
                if !bound {
                    return Ok(None);
                }
            }
        }
        Ok(Some((operative, token_id)))
    }

    /// Scan `[from, to]` for `AssetCreated` logs matching `(to == creator, channel)` and return
    /// the NEWEST `(operative, token_id_hex, block, log_index)`. Topics-only (the emitter
    /// contract can vary). Split-and-retry on a "range too large" signal — newest half first,
    /// so the freshly-minted asset surfaces with minimal RPC.
    fn scan_latest_asset_created(
        &self,
        network: &ChainNetwork,
        creator_topic: &str,
        channel_topic: &str,
        from: u64,
        to: u64,
    ) -> Result<Option<(String, String, u64, u64)>, Response> {
        let filter = json!({
            "fromBlock": format!("0x{from:x}"),
            "toBlock": format!("0x{to:x}"),
            "topics": [ASSET_CREATED_TOPIC0, creator_topic, channel_topic],
        });
        let logs = match self.evm_rpc_logs(network, filter) {
            Ok(logs) => logs,
            Err(response) => {
                if Self::is_range_limit_error(&response) && to.saturating_sub(from) >= MIN_LOG_RANGE
                {
                    let mid = from + (to - from) / 2;
                    // Newest-first: the upper half holds the freshest blocks.
                    if let Some(found) = self.scan_latest_asset_created(
                        network,
                        creator_topic,
                        channel_topic,
                        mid + 1,
                        to,
                    )? {
                        return Ok(Some(found));
                    }
                    return self.scan_latest_asset_created(
                        network,
                        creator_topic,
                        channel_topic,
                        from,
                        mid,
                    );
                }
                return Err(response);
            }
        };
        let entries = logs.as_array().ok_or_else(|| {
            Response::error(
                "upstream_invalid_logs",
                "eth_getLogs result was not an array",
            )
        })?;
        let mut best: Option<(String, String, u64, u64)> = None;
        for log in entries {
            let Some((operative, token_id, block_number, log_index)) =
                decode_asset_created_log(log)
            else {
                continue;
            };
            let newer = best
                .as_ref()
                .map(|(_, _, b, l)| (block_number, log_index) > (*b, *l))
                .unwrap_or(true);
            if newer {
                best = Some((operative, token_id, block_number, log_index));
            }
        }
        Ok(best)
    }

    /// Scan `[from, to]` for the `AssetCreated` whose MINT TRANSACTION embeds `want_content_id`
    /// (a normalised 32-hex `bytes16` KID) in `opRawData` — i.e. the EXACT asset just minted,
    /// not merely the channel's newest. For each candidate log (newest-first) we fetch the
    /// emitting transaction and decode its leading `bytes16` content id; the first KID match
    /// wins. Returns `(operative, token_id_hex)`. `None` when no log in the window matches (the
    /// mint of THIS asset is not on-chain yet → the caller keeps the trade step gated, #11).
    fn scan_asset_created_for_content_id(
        &self,
        network: &ChainNetwork,
        creator_topic: &str,
        channel_topic: &str,
        want_content_id: &str,
        from: u64,
        to: u64,
    ) -> Result<Option<(String, String)>, Response> {
        let entries =
            self.fetch_asset_created_logs(network, creator_topic, channel_topic, from, to)?;
        // Decode + sort newest-first so a repeated KID (re-mint) resolves to the latest one.
        let mut decoded: Vec<(String, String, u64, u64, String)> = Vec::new();
        for log in &entries {
            let Some((operative, token_id, block, log_index)) = decode_asset_created_log(log)
            else {
                continue;
            };
            let Some(tx_hash) = log.get("transactionHash").and_then(Value::as_str) else {
                continue;
            };
            decoded.push((operative, token_id, block, log_index, tx_hash.to_string()));
        }
        decoded.sort_by(|a, b| (b.2, b.3).cmp(&(a.2, a.3)));
        // Pre-fetch every candidate's mint calldata (live) BEFORE binding — the SAME MKT-1
        // fail-closed-by-omission rule the buy resolver applies (ESP-1). `decoded` is newest-first
        // and a re-mint reuses the KID, so silently skipping a candidate whose calldata is
        // unavailable could fall through the newest mint to an OLDER same-KID binder — a wrong
        // operative / wrong token-id trade-approval window. If ANY candidate's calldata cannot be
        // fetched we abort the whole resolve rather than decide on a partial candidate set.
        let mut inputs = std::collections::HashMap::new();
        for (_, _, _, _, tx_hash) in &decoded {
            if inputs.contains_key(tx_hash) {
                continue;
            }
            match self.tx_input(network, tx_hash)? {
                Some(input) => {
                    inputs.insert(tx_hash.clone(), input);
                }
                None => {
                    return Err(Response::error(
                        "candidate_input_unavailable",
                        &format!(
                            "mint calldata for candidate tx {tx_hash} is unavailable; refusing to resolve on a partial candidate set (fail-closed)"
                        ),
                    ));
                }
            }
        }
        // Newest-first: the first candidate whose calldata binds the KID wins. Precise decode for
        // the canonical runtime mint; fall back to a content-bound substring match for
        // RELAYED/forwarded mints whose OUTER ABI differs (the KID is still embedded in the
        // calldata, just not at the canonical opRawData head offset).
        for (operative, token_id, _, _, tx_hash) in decoded {
            let input = &inputs[&tx_hash];
            let precise = decode_mint_content_id(input)
                .and_then(|cid| normalize_content_id_bytes16(&cid))
                .as_deref()
                == Some(want_content_id);
            if precise || mint_input_binds_content_id(input, want_content_id) {
                return Ok(Some((operative, token_id)));
            }
        }
        Ok(None)
    }

    /// Fetch ALL `AssetCreated` log entries in `[from, to]` for `(creator, channel)`, splitting
    /// the range on a provider "range too large" signal (mirrors `scan_latest_asset_created`).
    /// The caller orders/filters the raw entries.
    fn fetch_asset_created_logs(
        &self,
        network: &ChainNetwork,
        creator_topic: &str,
        channel_topic: &str,
        from: u64,
        to: u64,
    ) -> Result<Vec<Value>, Response> {
        let filter = json!({
            "fromBlock": format!("0x{from:x}"),
            "toBlock": format!("0x{to:x}"),
            "topics": [ASSET_CREATED_TOPIC0, creator_topic, channel_topic],
        });
        match self.evm_rpc_logs(network, filter) {
            Ok(logs) => Ok(logs
                .as_array()
                .ok_or_else(|| {
                    Response::error(
                        "upstream_invalid_logs",
                        "eth_getLogs result was not an array",
                    )
                })?
                .clone()),
            Err(response) => {
                if Self::is_range_limit_error(&response) && to.saturating_sub(from) >= MIN_LOG_RANGE
                {
                    let mid = from + (to - from) / 2;
                    let mut entries = self.fetch_asset_created_logs(
                        network,
                        creator_topic,
                        channel_topic,
                        mid + 1,
                        to,
                    )?;
                    entries.extend(self.fetch_asset_created_logs(
                        network,
                        creator_topic,
                        channel_topic,
                        from,
                        mid,
                    )?);
                    Ok(entries)
                } else {
                    Err(response)
                }
            }
        }
    }

    /// Read a transaction's `input` calldata by hash (`eth_getTransactionByHash`). `None` when
    /// the transaction is unknown/pending or carries no input — the caller skips that candidate.
    fn tx_input(&self, network: &ChainNetwork, tx_hash: &str) -> Result<Option<String>, Response> {
        let result = self.evm_rpc(network, "eth_getTransactionByHash", json!([tx_hash]))?;
        Ok(result
            .get("input")
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// Read a channel's `authority()` (the gateway). `None` on any read miss so the caller can
    /// fall back to the configured default gateway (a non-fatal best-effort read).
    fn read_authority(&self, network: &ChainNetwork, channel: &str) -> Option<String> {
        let result = self
            .evm_rpc(
                network,
                "eth_call",
                json!([{ "to": channel, "data": AUTHORITY_SELECTOR }, "latest"]),
            )
            .ok()?;
        let raw = result.as_str()?;
        let word = decode_hex(raw, Some(32), "authority result").ok()?;
        word_to_address(&word).ok()
    }

    /// Read `isApprovedForAll(account, operator)` on an operative contract. Propagates a real
    /// RPC error (the caller fails closed) rather than guessing approval state.
    fn read_is_approved_for_all(
        &self,
        network: &ChainNetwork,
        operative: &str,
        account: &str,
        operator: &str,
    ) -> Result<bool, Response> {
        let data =
            encode_is_approved_for_all_calldata(IS_APPROVED_FOR_ALL_SELECTOR, account, operator)
                .map_err(|err| Response::error("invalid_call", &err))?;
        let result = self.evm_rpc(
            network,
            "eth_call",
            json!([{ "to": operative, "data": data }, "latest"]),
        )?;
        decode_evm_bool(&result).map_err(|err| Response::error("upstream_invalid_bool", &err))
    }

    fn broadcast_transaction(&self, network_id: &str, signed_transaction: &str) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        if let Err(err) = validate_signed_transaction(signed_transaction) {
            return Response::error("invalid_signed_transaction", &err);
        }
        match self.evm_rpc(
            network,
            "eth_sendRawTransaction",
            json!([signed_transaction]),
        ) {
            Ok(hash) => {
                let Some(hash) = hash.as_str() else {
                    return Response::error(
                        "upstream_invalid_hash",
                        "transaction hash must be hex",
                    );
                };
                if let Err(err) = validate_evm_hash(hash) {
                    return Response::error("upstream_invalid_hash", &err);
                }
                Response::ok(json!({
                    "schema": "elastos.chain.broadcast_receipt/v1",
                    "network": network.id,
                    "transaction_hash": hash,
                }))
            }
            Err(response) => response,
        }
    }

    fn node_lifecycle(&mut self, network_id: &str, action: NodeLifecycleAction) -> Response {
        let network = match self.network_for_status(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        let loopback = network.rpc_url.starts_with("http://127.0.0.1:")
            || network.rpc_url.starts_with("http://localhost:");
        let supervisor = self.node_supervisor.networks.get(&network.id).cloned();
        let control_available = loopback && supervisor.is_some();
        let managed = loopback;
        let state = if network.rpc_url.trim().is_empty() {
            NodeLifecycleStateKind::NotConfigured
        } else if control_available {
            NodeLifecycleStateKind::ManagedLocal
        } else if loopback {
            NodeLifecycleStateKind::ExternalLoopback
        } else {
            NodeLifecycleStateKind::RemoteBackend
        };
        let network_id = network.id.clone();
        let network = network.public_view();
        if action != NodeLifecycleAction::Status && !control_available {
            return Response::error(
                "managed_node_unavailable",
                "local node lifecycle control is not configured for this network",
            );
        }
        if action != NodeLifecycleAction::Status {
            let Some(supervisor) = supervisor.as_ref() else {
                return Response::error(
                    "managed_node_unavailable",
                    "local node lifecycle control is not configured for this network",
                );
            };
            if let Err(response) = run_node_supervisor_action(supervisor, action) {
                return response;
            }
        }
        let persisted = match self.persist_node_lifecycle_state(&network_id, state, managed) {
            Ok(persisted) => persisted,
            Err(response) => return response,
        };
        Response::ok(json!({
            "schema": "elastos.chain.node_lifecycle/v1",
            "network": network,
            "managed": persisted.managed,
            "control_available": control_available,
            "control_reason": if control_available { "operator-approved supervisor configured" } else { NODE_LIFECYCLE_CONTROL_REASON },
            "action": action,
            "state": persisted.state,
            "first_seen_at": persisted.first_seen_at,
            "updated_at": persisted.updated_at,
        }))
    }

    fn persist_node_lifecycle_state(
        &mut self,
        network_id: &str,
        state: NodeLifecycleStateKind,
        managed: bool,
    ) -> Result<PersistedNodeLifecycleState, Response> {
        if let Some(err) = &self.node_lifecycle_state_error {
            return Err(Response::error("node_lifecycle_state_unavailable", err));
        }
        let now = now_ts();
        let entry = self
            .node_lifecycle_state
            .networks
            .entry(network_id.to_string())
            .and_modify(|entry| {
                entry.state = state;
                entry.managed = managed;
                entry.updated_at = now;
            })
            .or_insert_with(|| PersistedNodeLifecycleState {
                state,
                managed,
                first_seen_at: now,
                updated_at: now,
            })
            .clone();
        write_node_lifecycle_state_file(
            &self.node_lifecycle_state_path,
            &self.node_lifecycle_state,
        )
        .map_err(|err| Response::error("node_lifecycle_state_unavailable", &err))?;
        Ok(entry)
    }
}

/// Does this RPC error represent a CONTRACT REVERT (a definitive on-chain "no" for the call)
/// rather than a transport/endpoint failure? Used by the rights read to mirror PC2's
/// `.catch(() => false)`: an `eth_call` that reverts means the holder has no access for the
/// queried contentId (often: the content is not registered on the gateway), which must fail
/// CLOSED as `has_access: false` — not bubble up as a 503 outage. Standard JSON-RPC revert
/// signals: code `3` (EIP-1474 "execution reverted") and/or an "execution reverted" message.
fn is_contract_revert(resp: &Response) -> bool {
    match resp {
        Response::Error { code, message } if code == "upstream_rpc_error" => {
            let m = message.to_ascii_lowercase();
            m.contains("execution reverted")
                || m.contains("revert")
                || message.contains("\"code\":3")
        }
        _ => false,
    }
}

fn main() {
    eprintln!("chain-provider: starting v{} (typed RPC)", PROVIDER_VERSION);

    let info = CapsuleInfo::from_env();
    if info.is_elastos_runtime() {
        eprintln!("Running as: {} ({})", info.name(), info.id());
    }

    let mut provider = ChainProvider::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("chain-provider read error: {}", err);
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(err) => {
                let response = Response::error("invalid_request", &err.to_string());
                writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
                stdout.flush().unwrap();
                continue;
            }
        };
        let is_shutdown = matches!(request, Request::Shutdown);
        let response = provider.handle(request);
        writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
        stdout.flush().unwrap();
        if is_shutdown {
            break;
        }
    }

    eprintln!("chain-provider exiting");
}
