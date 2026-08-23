//! ElastOS Chain Provider Capsule
//!
//! Typed chain access for Elastos and node-backed networks.
//! Apps never receive raw RPC URLs or arbitrary JSON-RPC passthrough.

use elastos_guest::prelude::*;
use elastos_protected_content_contracts::{
    CanonicalContract, EvmContractAddressV1, EvmFunctionSelectorV1, EvmRightsMethodAbiV1,
    RightsEvaluationEvidenceV1, RightsObservationFinalityV1, RightsPolicyBodyV1,
    RightsSubjectSourceV1, RuntimeOperationIssuerKeyV1, SignedRuntimeReleaseOperationV1,
    MAX_RIGHTS_EVIDENCE_LIFETIME_SECS,
};
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
const MAX_PROTECTED_CONTENT_RUNTIME_OPERATION_BYTES: usize = 16384;

struct ChainProvider {
    networks: Vec<ChainNetwork>,
    client: reqwest::blocking::Client,
    node_lifecycle_state_path: PathBuf,
    node_lifecycle_state: NodeLifecycleStateFile,
    node_lifecycle_state_error: Option<String>,
    node_supervisor: NodeSupervisorConfig,
    protected_content_runtime_issuer: Option<RuntimeOperationIssuerKeyV1>,
    now_unix_seconds: fn() -> u64,
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
            protected_content_runtime_issuer: None,
            now_unix_seconds: now_ts,
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
            Request::ProtectedContentRightsEvidence {
                signed_runtime_release_operation,
            } => self.protected_content_rights_evidence(&signed_runtime_release_operation),
            Request::ResolveProtectedContentPolicy { content_id, action } => {
                self.resolve_protected_content_policy(&content_id, action)
            }
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
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, config: Value) -> Response {
        let extra = config.get("extra").unwrap_or(&config);
        let next_networks = if let Some(networks) = config
            .get("extra")
            .and_then(|extra| extra.get("networks"))
            .or_else(|| config.get("networks"))
        {
            match serde_json::from_value::<Vec<ChainNetwork>>(networks.clone()) {
                Ok(networks) => {
                    if let Err(err) = validate_networks(&networks) {
                        return Response::error("invalid_config", &err);
                    }
                    Some(networks)
                }
                Err(err) => return Response::error("invalid_config", &err.to_string()),
            }
        } else {
            None
        };
        let next_supervisor = if let Some(supervisor) = extra.get("node_supervisor") {
            match serde_json::from_value::<NodeSupervisorConfig>(supervisor.clone()) {
                Ok(supervisor) => {
                    if let Err(err) = validate_node_supervisor_config(&supervisor) {
                        return Response::error("invalid_config", &err);
                    }
                    Some(supervisor)
                }
                Err(err) => return Response::error("invalid_config", &err.to_string()),
            }
        } else {
            None
        };
        let next_runtime_issuer = if let Some(runtime_issuer) = config
            .get("extra")
            .and_then(|extra| extra.get("protected_content_runtime_issuer"))
        {
            match parse_runtime_issuer(runtime_issuer) {
                Ok(runtime_issuer) => Some(runtime_issuer),
                Err(err) => return Response::error("invalid_config", &err),
            }
        } else {
            None
        };
        if let Some(networks) = next_networks {
            self.networks = networks;
        }
        if let Some(supervisor) = next_supervisor {
            self.node_supervisor = supervisor;
        }
        self.protected_content_runtime_issuer = next_runtime_issuer;
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

    fn protected_content_rights_evidence(
        &self,
        signed_runtime_release_operation: &str,
    ) -> Response {
        let operation = match decode_contract_hex::<SignedRuntimeReleaseOperationV1>(
            signed_runtime_release_operation,
            MAX_PROTECTED_CONTENT_RUNTIME_OPERATION_BYTES,
            "signed_runtime_release_operation",
        ) {
            Ok(operation) => operation,
            Err(err) => return Response::error("invalid_runtime_operation", &err),
        };
        let expected_runtime_issuer = match self.protected_content_runtime_issuer {
            Some(issuer) => issuer,
            None => {
                return Response::error(
                    "runtime_issuer_not_configured",
                    "protected-content Runtime issuer is not configured",
                )
            }
        };
        let now = (self.now_unix_seconds)();
        let authenticated = match operation.verify(expected_runtime_issuer, now) {
            Ok(authenticated) => authenticated,
            Err(_) => {
                return Response::error(
                    "invalid_runtime_operation",
                    "signed Runtime operation is invalid",
                )
            }
        };
        let policy = operation.statement().policy_body();
        let request = operation.statement().evidence_request();
        if let Err(err) = request.validate_against_policy(policy) {
            return Response::error("invalid_runtime_operation", &err.to_string());
        }
        if policy.subject_source() != RightsSubjectSourceV1::WalletAddress {
            return Response::error(
                "unsupported_rights_subject_source",
                "rights subject source is not supported",
            );
        }
        let network = match self.evm_network_for_protected_content_policy(policy) {
            Ok(network) => network,
            Err(response) => return response,
        };
        let chain_id = policy.chain_id();
        let method = match policy.method_abi() {
            EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString => {
                let contract = format!("0x{}", encode_hex(policy.contract_address().as_bytes()));
                match rights_method(network, "has_access_by_content_id", &contract) {
                    Ok(method)
                        if method.selector
                            == format!(
                                "0x{}",
                                encode_hex(policy.function_selector().as_bytes())
                            ) =>
                    {
                        method
                    }
                    Ok(_) => {
                        return Response::error(
                            "rights_selector_mismatch",
                            "configured rights method selector does not match policy",
                        )
                    }
                    Err(response) => return response,
                }
            }
        };
        match self.evm_rpc(network, "eth_chainId", json!([])) {
            Ok(value) => match value.as_str().and_then(|value| parse_hex_u64(value).ok()) {
                Some(live_chain_id) if live_chain_id == chain_id => {}
                Some(_) => {
                    return Response::error(
                        "chain_id_mismatch",
                        "live chain id does not match policy chain id",
                    )
                }
                None => {
                    return Response::error(
                        "upstream_invalid_chain_id",
                        "live chain id must be a hex quantity",
                    )
                }
            },
            Err(response) => return response,
        }

        let head = match self.evm_rpc(network, "eth_blockNumber", json!([])) {
            Ok(value) => match value.as_str().and_then(|value| parse_hex_u64(value).ok()) {
                Some(head) => head,
                None => {
                    return Response::error(
                        "upstream_invalid_head",
                        "chain head must be a hex quantity",
                    )
                }
            },
            Err(response) => return response,
        };
        let min_confirmations = u64::from(policy.observation_finality().min_confirmations());
        let observed = match head.checked_sub(min_confirmations) {
            Some(observed) => observed,
            None => {
                return Response::error(
                    "insufficient_finality",
                    "chain head is below required confirmation depth",
                )
            }
        };
        let observed_tag = format!("0x{observed:x}");
        let observed_hash = match self.evm_rpc(
            network,
            "eth_getBlockByNumber",
            json!([observed_tag.as_str(), false]),
        ) {
            Ok(value) => match evm_observed_block(&value, observed) {
                Ok(hash) => hash,
                Err(err) => return Response::error("upstream_invalid_block", &err),
            },
            Err(response) => return response,
        };

        let subject = format!("0x{}", encode_hex(request.binding().wallet().as_bytes()));
        let data = match method.abi {
            RightsMethodAbi::HasAccessByContentIdStringAddressString => {
                match encode_has_access_by_content_id_call(
                    &method.selector,
                    policy.content_id(),
                    &subject,
                    policy.evm_right_argument(),
                ) {
                    Ok(data) => data,
                    Err(err) => return Response::error("invalid_rights_method", &err),
                }
            }
        };
        let has_access = match self.evm_rpc(
            network,
            "eth_call",
            json!([
                { "to": method.contract.as_str(), "data": data },
                {
                    "blockHash": format!("0x{}", encode_hex(observed_hash.as_bytes())),
                    "requireCanonical": true
                }
            ]),
        ) {
            Ok(result) => match decode_evm_bool(&result) {
                Ok(has_access) => has_access,
                Err(err) => return Response::error("upstream_invalid_bool", &err),
            },
            Err(response) => return response,
        };
        let acquired_at = now;
        let expires_at = match acquired_at
            .checked_add(MAX_RIGHTS_EVIDENCE_LIFETIME_SECS)
            .map(|expires_at| expires_at.min(operation.statement().expires_at()))
        {
            Some(expires_at) if expires_at > acquired_at => expires_at,
            _ => {
                return Response::error(
                    "invalid_evidence_window",
                    "rights evidence window is outside Runtime operation",
                )
            }
        };
        let evidence = match RightsEvaluationEvidenceV1::new(
            authenticated.operation_hash(),
            authenticated.release_request_hash(),
            request.binding().clone(),
            request.policy_identity().clone(),
            request.binding().wallet(),
            chain_id,
            observed,
            observed_hash,
            head,
            has_access,
            acquired_at,
            expires_at,
        )
        .and_then(|evidence| {
            evidence.validate_against_request(request, policy)?;
            Ok(evidence)
        }) {
            Ok(evidence) => evidence,
            Err(err) => return Response::error("invalid_evidence", &err.to_string()),
        };
        let evidence_bytes = match evidence.canonical_bytes() {
            Ok(bytes) => bytes,
            Err(err) => return Response::error("invalid_evidence", &err.to_string()),
        };
        let evidence_hash = match evidence.canonical_hash() {
            Ok(hash) => hash,
            Err(err) => return Response::error("invalid_evidence", &err.to_string()),
        };
        Response::ok(json!({
            "schema": "elastos.chain.protected-content-rights-evidence/v1",
            "chain_id": chain_id,
            "observed_block_number": observed,
            "head_block_number": head,
            "observed_block_hash": format!("0x{}", encode_hex(observed_hash.as_bytes())),
            "rights_evaluation_evidence": format!("0x{}", encode_hex(&evidence_bytes)),
            "rights_evaluation_evidence_hash": format!("0x{}", encode_hex(evidence_hash.as_bytes())),
        }))
    }

    fn resolve_protected_content_policy(
        &self,
        content_id: &str,
        action: ProtectedContentPolicyAction,
    ) -> Response {
        let (network, method, policy_source) =
            match self.configured_protected_content_policy_source(action) {
                Ok(source) => source,
                Err(response) => return response,
            };
        let Some(chain_id) = network.chain_id else {
            return Response::error(
                "rights_policy_not_configured",
                "no configured protected-content rights policy source matches action",
            );
        };
        let contract = match parse_evm_contract_address(&method.contract) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let selector = match parse_evm_function_selector(&method.selector) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let policy = match RightsPolicyBodyV1::new(
            content_id,
            action.to_contract_action(),
            policy_source.right_argument.as_str(),
            RightsSubjectSourceV1::WalletAddress,
            chain_id,
            contract,
            selector,
            method.abi.to_contract_abi(),
            RightsObservationFinalityV1::new(policy_source.min_confirmations),
        ) {
            Ok(value) => value,
            Err(_) => {
                return Response::error(
                    "invalid_rights_policy_request",
                    "protected-content rights policy request is invalid",
                )
            }
        };
        let policy_bytes = match policy.canonical_bytes() {
            Ok(value) => value,
            Err(_) => {
                return Response::error(
                    "invalid_rights_policy_request",
                    "protected-content rights policy request is invalid",
                )
            }
        };
        Response::ok(json!({
            "schema": PROTECTED_CONTENT_POLICY_SCHEMA,
            "policy_body": format!("0x{}", encode_hex(&policy_bytes)),
        }))
    }

    fn evm_network_for_protected_content_policy(
        &self,
        policy: &RightsPolicyBodyV1,
    ) -> Result<&ChainNetwork, Response> {
        let contract = format!("0x{}", encode_hex(policy.contract_address().as_bytes()));
        let mut matches = self.networks.iter().filter(|network| {
            network.kind == ChainKind::EvmJsonRpc
                && network.chain_id == Some(policy.chain_id())
                && network.rights_methods.iter().any(|method| {
                    method.id == "has_access_by_content_id"
                        && method.contract.eq_ignore_ascii_case(&contract)
                })
        });
        let Some(network) = matches.next() else {
            return Err(Response::error(
                "rights_query_not_configured",
                "no configured protected-content rights evidence source matches policy",
            ));
        };
        if matches.next().is_some() {
            return Err(Response::error(
                "ambiguous_rights_evidence_source",
                "multiple protected-content rights evidence sources match policy",
            ));
        }
        Ok(network)
    }

    fn configured_protected_content_policy_source(
        &self,
        action: ProtectedContentPolicyAction,
    ) -> Result<(&ChainNetwork, &RightsMethod, &ProtectedContentPolicySource), Response> {
        let mut matches = self
            .networks
            .iter()
            .filter(|network| network.kind == ChainKind::EvmJsonRpc)
            .flat_map(|network| {
                network.rights_methods.iter().flat_map(move |method| {
                    method
                        .protected_content_policies
                        .iter()
                        .filter(move |policy| policy.action == action)
                        .map(move |policy| (network, method, policy))
                })
            });
        let Some(source) = matches.next() else {
            return Err(Response::error(
                "rights_policy_not_configured",
                "no configured protected-content rights policy source matches action",
            ));
        };
        if matches.next().is_some() {
            return Err(Response::error(
                "ambiguous_rights_policy_source",
                "multiple protected-content rights policy sources match action",
            ));
        }
        Ok(source)
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

fn parse_evm_contract_address(value: &str) -> Result<EvmContractAddressV1, Response> {
    let bytes = decode_hex(value, Some(20), "EVM address").map_err(|_| {
        Response::error(
            "invalid_configured_policy_source",
            "configured protected-content rights policy source is invalid",
        )
    })?;
    let bytes: [u8; 20] = bytes.try_into().map_err(|_| {
        Response::error(
            "invalid_configured_policy_source",
            "configured protected-content rights policy source is invalid",
        )
    })?;
    EvmContractAddressV1::new(bytes).map_err(|_| {
        Response::error(
            "invalid_configured_policy_source",
            "configured protected-content rights policy source is invalid",
        )
    })
}

fn parse_evm_function_selector(value: &str) -> Result<EvmFunctionSelectorV1, Response> {
    let bytes = decode_hex(value, Some(4), "EVM function selector").map_err(|_| {
        Response::error(
            "invalid_configured_policy_source",
            "configured protected-content rights policy source is invalid",
        )
    })?;
    let bytes: [u8; 4] = bytes.try_into().map_err(|_| {
        Response::error(
            "invalid_configured_policy_source",
            "configured protected-content rights policy source is invalid",
        )
    })?;
    EvmFunctionSelectorV1::new(bytes).map_err(|_| {
        Response::error(
            "invalid_configured_policy_source",
            "configured protected-content rights policy source is invalid",
        )
    })
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
