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
mod config;
mod lifecycle;
mod protocol;
mod rpc;
mod validation;

#[cfg(test)]
mod tests;

use abi::*;
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

struct ChainProvider {
    networks: Vec<ChainNetwork>,
    client: reqwest::blocking::Client,
    node_lifecycle_state_path: PathBuf,
    node_lifecycle_state: NodeLifecycleStateFile,
    node_lifecycle_state_error: Option<String>,
    node_supervisor: NodeSupervisorConfig,
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
        Self {
            networks: default_networks(),
            client,
            node_lifecycle_state_path,
            node_lifecycle_state,
            node_lifecycle_state_error,
            node_supervisor: NodeSupervisorConfig::default(),
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
        match self.evm_rpc(network, "eth_getLogs", json!([filter])) {
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
        match self.evm_rpc(
            network,
            "eth_call",
            json!([{ "to": method.contract.as_str(), "data": data }, "latest"]),
        ) {
            Ok(result) => match decode_evm_bool(&result) {
                Ok(has_access) => Response::ok(json!({
                    "network": network.id,
                    "contract": method.contract.as_str(),
                    "content_id": content_id,
                    "subject": subject,
                    "right": right,
                    "has_access": has_access,
                })),
                Err(err) => Response::error("upstream_invalid_bool", &err),
            },
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
