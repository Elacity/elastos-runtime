//! ElastOS Chain Provider Capsule
//!
//! Typed chain access for Elastos and node-backed networks.
//! Apps never receive raw RPC URLs or arbitrary JSON-RPC passthrough.

use crate::backends::EVM_RPC_BATCH_MAX;
use elastos_guest::prelude::*;
use elastos_protected_content_contracts::{
    CanonicalContract, ContentAccessIdV1, Digest32, EncryptedContentIdentityV1,
    EvmContractAddressV1, EvmFunctionSelectorV1, RightsEvaluationEvidenceV1,
    RightsObservationFinalityV1, RightsPolicyBodyV1, RightsSubjectSourceV1,
    RuntimeOperationIssuerKeyV1, SignedRuntimeReleaseOperationV1,
    MAX_RIGHTS_EVIDENCE_LIFETIME_SECS,
};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

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
// Source-backed creator mode for protected content. The access method is the
// creator's to choose (`ProtectedContentAccessMethod` below); these are the
// parts that do not vary with it:
// - ACCESS_TOKEN role 1
// - ROYALTY_SHARE role 2
// - creator royalty 950 ERC-1155 ROYALTY_SHARE units (tokenId 2). 1000 units
//   exist per asset: the creator's 950 (95%) plus the protocol owner's share
//   minted by the contracts from `CentralStorage.protocolShares()` — 50 (5%)
//   on the deployed Base 8453 config (verified on-chain, ELACITY-2296).

/// How a creator lets people reach an asset, in the `opType` the channel takes.
///
/// One type rather than three scattered comparisons on a `u16`, because the
/// same distinction decides three separate things -- what `opRawData` holds,
/// whether a listing is created, and what the mint receipt may be expected to
/// prove -- and those answers must not drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProtectedContentAccessMethod {
    /// No operative, no listing, no access token. `AssetFactory` skips its
    /// whole operative branch for op type 0 and only binds the content id.
    Free,
    /// One sale per access token, no resale.
    BuyOnce,
    /// Sale plus resale, which is the one method whose `opRawData` carries a
    /// trailing `uint16 resellerCut`.
    BuyAndResell,
}

impl ProtectedContentAccessMethod {
    pub(crate) const fn from_code(code: u16) -> Option<Self> {
        match code {
            0 => Some(Self::Free),
            1 => Some(Self::BuyOnce),
            2 => Some(Self::BuyAndResell),
            _ => None,
        }
    }

    pub(crate) const fn code(self) -> u16 {
        match self {
            Self::Free => 0,
            Self::BuyOnce => 1,
            Self::BuyAndResell => 2,
        }
    }

    /// Whether this method sells anything -- which is also whether it creates
    /// an operative, a listing and a royalty split, since the contracts tie
    /// all four to the same `opType > 0` branch.
    pub(crate) const fn is_paid(self) -> bool {
        !matches!(self, Self::Free)
    }
}

/// The pay token a mint prices in, and what its price means.
///
/// Returned together because they are one fact: an address without its
/// decimals is a number nobody can interpret, and the pair is what both the
/// encoding and the price check need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedPayToken {
    address: String,
    decimals: u8,
}

/// The default pay token, and the set a creator may choose from.
///
/// The configured list is the offer, first entry first, and an empty list
/// means the legacy single token at eighteen decimals -- which is what every
/// deployment meant before a creator could choose one.
fn protected_content_pay_token_choices(
    mint: &ProtectedContentCreatorMintMethod,
) -> Vec<ResolvedPayToken> {
    if mint.pay_tokens.is_empty() {
        return vec![ResolvedPayToken {
            address: normalize_evm_address(&mint.pay_token),
            decimals: PROTECTED_CONTENT_NATIVE_PAY_TOKEN_DECIMALS,
        }];
    }
    mint.pay_tokens
        .iter()
        .map(|token| ResolvedPayToken {
            address: normalize_evm_address(&token.address),
            decimals: token.decimals,
        })
        .collect()
}

/// What each offered pay token is called, in the same order as the choices.
/// Display only -- the address is what is encoded and what is checked.
fn protected_content_pay_token_symbols(mint: &ProtectedContentCreatorMintMethod) -> Vec<String> {
    if mint.pay_tokens.is_empty() {
        return vec![PROTECTED_CONTENT_NATIVE_PAY_TOKEN_SYMBOL.to_string()];
    }
    mint.pay_tokens
        .iter()
        .map(|token| token.symbol.clone())
        .collect()
}

/// The pay token a request names, or the default when it names none.
///
/// An address this deployment does not offer is refused rather than encoded:
/// the price is an integer of that token's smallest unit, so a token whose
/// decimals are unknown is a price whose meaning is unknown.
fn resolve_protected_content_pay_token(
    mint: &ProtectedContentCreatorMintMethod,
    requested: Option<&str>,
) -> Result<ResolvedPayToken, String> {
    let choices = protected_content_pay_token_choices(mint);
    let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return choices
            .first()
            .cloned()
            .ok_or_else(|| "no pay token is configured for this mint source".to_string());
    };
    validate_evm_address(requested)?;
    let requested = normalize_evm_address(requested);
    choices
        .into_iter()
        .find(|choice| choice.address == requested)
        .ok_or_else(|| format!("pay token {requested} is not offered by this mint source"))
}

/// The chain's own coin, and every pay token a deployment carried before the
/// list existed.
const PROTECTED_CONTENT_NATIVE_PAY_TOKEN_DECIMALS: u8 = 18;
/// What a chain's own coin is called when a deployment offers no list. Every
/// EVM this Runtime mints on prices its coin at eighteen decimals; the name
/// differs, so this is the neutral one.
const PROTECTED_CONTENT_NATIVE_PAY_TOKEN_SYMBOL: &str = "native";

/// A resale cut is carried in deci-percent, so 900 is 90%. The whole sale is
/// the ceiling; anything above it is not a share.
const PROTECTED_CONTENT_RESELLER_CUT_MAX: u16 = 1000;
const PROTECTED_CONTENT_CREATOR_ACCESS_TOKEN_ROLE: u64 = 1;
const PROTECTED_CONTENT_CREATOR_ROYALTY_SHARE_ROLE: u64 = 2;
const PROTECTED_CONTENT_CREATOR_ROYALTY_SHARE_UNITS: &str = "0x3b6";
/// The creator's whole share in `ROYALTY_SHARE` units, matching the hex
/// constant above. One unit is 0.1% of the sale.
const PROTECTED_CONTENT_CREATOR_ROYALTY_UNITS: u32 = 950;
/// The media token's own URI, derived from the metadata DIRECTORY base.
///
/// One mint carries the same CID twice, in two shapes that must not be
/// confused. The operative's base URI stays the folder, because
/// `OperativePrimitive.uri(id)` appends `/{id}.json` to whatever it is given.
/// The media token's URI has to name the document itself, because a
/// marketplace dereferences it directly rather than appending to it -- a
/// folder there is what made published listings arrive with no title, no type
/// and no price.
///
/// `AssetCreated._uri` echoes this exact value (`StandardChannel` emits the
/// mint call's first argument), so the receipt corroboration compares against
/// what this returns and never against the base it was derived from.
fn protected_content_media_token_uri(base: &str) -> String {
    format!("{}/metadata.json", base.trim_end_matches('/'))
}
/// One page of the Shops surface. A request larger than this is a caller
/// asking for something the surface does not show.
const PROTECTED_CONTENT_CHANNEL_ACCESS_MAX: usize = 64;
/// How many of a channel-access read's requests are in flight at once. Enough
/// that a page of channels resolves while a person is still looking at it,
/// bounded so that this Home is not the reason a public endpoint rate-limits
/// it.
const PROTECTED_CONTENT_CHANNEL_ACCESS_WORKERS: usize = 8;
const PROTECTED_CONTENT_PURCHASE_ACCESS_MAX_FINALIZED_AGE_SECS: u64 = 30 * 60;
const PROTECTED_CONTENT_PURCHASE_ACCESS_MAX_FUTURE_SKEW_SECS: u64 = 30;
const PROTECTED_CONTENT_UNBOUND_CONTENT_ID_SELECTOR: [u8; 4] = [0xca, 0xd8, 0x82, 0x23];

struct ChainProvider {
    networks: Vec<ChainNetwork>,
    protected_content_network_id: Option<String>,
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
            protected_content_network_id: None,
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
            Request::ResolveProtectedContentPolicy {
                encrypted_content,
                content_access_id,
                action,
            } => self.resolve_protected_content_policy(
                &encrypted_content,
                &content_access_id,
                action,
            ),
            Request::DescribeProtectedContentCreatorMintSource => {
                self.describe_protected_content_creator_mint_source()
            }
            Request::DescribeProtectedContentMarketSource { network } => {
                self.describe_protected_content_market_source(&network)
            }
            Request::ResolveProtectedContentCreatorMint {
                creator,
                token_uri,
                content_access_id,
                copies,
                price,
                royalties,
                op_type_code,
                reseller_cut,
                ledger,
                pay_token,
            } => self.resolve_protected_content_creator_mint(
                &creator,
                &token_uri,
                &content_access_id,
                &copies,
                &price,
                &royalties,
                op_type_code,
                reseller_cut,
                ledger.as_deref(),
                pay_token.as_deref(),
            ),
            Request::ResolveProtectedContentMintReceipt {
                network,
                hash,
                creator,
                ledger,
                token_uri,
                op_type_code,
            } => self.resolve_protected_content_mint_receipt(
                &network,
                &hash,
                &creator,
                &ledger,
                &token_uri,
                op_type_code,
            ),
            Request::ResolveProtectedContentVerifiedListing {
                network,
                seller,
                ledger,
                token_id,
            } => self
                .resolve_protected_content_verified_listing(&network, &seller, &ledger, &token_id),
            Request::ResolveProtectedContentPaymentProcessor { network, operative } => {
                self.resolve_protected_content_payment_processor(&network, &operative)
            }
            Request::ResolveProtectedContentChannelAccess {
                network,
                account,
                channels,
            } => self.resolve_protected_content_channel_access(&network, &account, &channels),
            Request::ResolveProtectedContentPurchase {
                seller,
                chain_namespace,
                network,
                ledger,
                token_id,
            } => self.resolve_protected_content_purchase(
                &seller,
                &chain_namespace,
                &network,
                &ledger,
                &token_id,
            ),
            Request::ResolveProtectedContentPurchaseAccess {
                request_id,
                network,
                wallet,
                content_access_id,
            } => self.resolve_protected_content_purchase_access(
                &request_id,
                &network,
                &wallet,
                &content_access_id,
            ),
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
        let configured_networks = config
            .get("extra")
            .and_then(|extra| extra.get("networks"))
            .or_else(|| config.get("networks"));
        let configured_protected_content_network = extra.get("protected_content_network");
        if configured_networks.is_some() && configured_protected_content_network.is_some() {
            return Response::error(
                "invalid_config",
                "full networks and protected-content network configuration are ambiguous",
            );
        }
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
        let (next_networks, next_protected_content_network_id) =
            if let Some(networks) = configured_networks {
                if self.protected_content_network_id.is_some() {
                    return Response::error(
                        "invalid_config",
                        "protected-content network configuration is already active",
                    );
                }
                let networks = match serde_json::from_value::<Vec<ChainNetwork>>(networks.clone()) {
                    Ok(networks) => networks,
                    Err(err) => return Response::error("invalid_config", &err.to_string()),
                };
                if let Err(err) = validate_networks(&networks) {
                    return Response::error("invalid_config", &err);
                }
                if networks.iter().any(network_has_protected_content_source) {
                    return Response::error(
                        "invalid_config",
                        "protected-content sources require protected_content_network",
                    );
                }
                (Some(networks), None)
            } else if let Some(network) = configured_protected_content_network {
                if next_runtime_issuer.is_none() {
                    return Response::error(
                        "invalid_config",
                        "protected-content network requires a Runtime operation issuer",
                    );
                }
                if self.protected_content_network_id.is_some()
                    || self
                        .networks
                        .iter()
                        .any(network_has_protected_content_source)
                {
                    return Response::error(
                        "invalid_config",
                        "a protected-content network source is already configured",
                    );
                }
                let network = match serde_json::from_value::<ChainNetwork>(network.clone()) {
                    Ok(network) => network,
                    Err(err) => return Response::error("invalid_config", &err.to_string()),
                };
                if let Err(err) = validate_protected_content_network(&network) {
                    return Response::error("invalid_config", &err);
                }
                let matching_indices = self
                    .networks
                    .iter()
                    .enumerate()
                    .filter_map(|(index, current)| (current.id == network.id).then_some(index))
                    .collect::<Vec<_>>();
                if matching_indices.len() > 1 {
                    return Response::error(
                        "invalid_config",
                        "protected-content network matches more than one configured network",
                    );
                }
                let network_id = network.id.clone();
                let mut networks = self.networks.clone();
                if let Some(index) = matching_indices.first().copied() {
                    networks[index] = network;
                } else {
                    networks.push(network);
                }
                (Some(networks), Some(network_id))
            } else {
                (None, self.protected_content_network_id.clone())
            };
        if let Some(networks) = next_networks {
            self.networks = networks;
        }
        self.protected_content_network_id = next_protected_content_network_id;
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
        if request.binding().encrypted_content() != policy.encrypted_content() {
            return Response::error("invalid_runtime_operation", "policy binding mismatch");
        }
        if policy.subject_source() != RightsSubjectSourceV1::WalletAddress {
            return Response::error(
                "unsupported_rights_subject_source",
                "rights subject source is not supported",
            );
        }
        let (network, method, policy_source) =
            match self.configured_protected_content_policy_source_for_policy(policy) {
                Ok(source) => source,
                Err(response) => return response,
            };
        let subject = format!("0x{}", encode_hex(request.binding().wallet().as_bytes()));
        let data = match method.abi {
            RightsMethodAbi::HasAccessByContentIdAddressBytes16 => {
                match encode_has_access_by_content_id_call(
                    &method.selector,
                    policy.content_access_id().as_bytes(),
                    &subject,
                ) {
                    Ok(data) => data,
                    Err(err) => return Response::error("invalid_rights_method", &err),
                }
            }
        };
        let observation = match self.observe_protected_content_rights(
            network,
            &policy_source.evidence_rpc_urls,
            policy.chain_id(),
            ProtectedContentRightsCall {
                contract: &method.contract,
                data: &data,
                expected_content_access_id: &policy.content_access_id(),
                // Evidence that authorizes releasing a content key.
                // Irreversible once released, so it rests only on finalized
                // state.
                block: ProtectedContentRightsBlock::Finalized,
            },
        ) {
            Ok(observation) => observation,
            Err(response) => return response,
        };
        let has_access = match observation.outcome {
            ProtectedContentRightsObservationKind::HasAccess(has_access) => has_access,
            ProtectedContentRightsObservationKind::Unbound(_) => {
                return Response::error(
                    "unknown_protected_content_object",
                    "protected-content content access id is not bound on chain",
                )
            }
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
            observation.chain_id,
            observation.finalized_block_number,
            observation.finalized_block_hash,
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
            "chain_id": observation.chain_id,
            "finalized_block_number": observation.finalized_block_number,
            "finalized_block_hash": format!("0x{}", encode_hex(observation.finalized_block_hash.as_bytes())),
            "rights_evaluation_evidence": format!("0x{}", encode_hex(&evidence_bytes)),
            "rights_evaluation_evidence_hash": format!("0x{}", encode_hex(evidence_hash.as_bytes())),
        }))
    }

    fn resolve_protected_content_policy(
        &self,
        encrypted_content: &str,
        content_access_id: &str,
        action: ProtectedContentPolicyAction,
    ) -> Response {
        let (network, method, _policy_source) =
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
        let encrypted_content = match parse_encrypted_content_identity(encrypted_content) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let content_access_id = match parse_content_access_id(content_access_id) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let policy = match RightsPolicyBodyV1::new(
            encrypted_content,
            content_access_id,
            action.to_contract_action(),
            RightsSubjectSourceV1::WalletAddress,
            chain_id,
            contract,
            selector,
            method.abi.to_contract_abi(),
            RightsObservationFinalityV1::finalized(),
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

    /// The market contract a creator must authorise as ERC-1155 operator on
    /// the ledger before a buyer can be delivered a copy: the Runtime checks
    /// and, if needed, raises that approval in the creator tail.
    fn describe_protected_content_market_source(&self, network_id: &str) -> Response {
        let market = match self.configured_protected_content_market_source(network_id) {
            Ok(market) => market,
            Err(response) => return response,
        };
        Response::ok(json!({
            "schema": PROTECTED_CONTENT_MARKET_SOURCE_SCHEMA,
            "network": network_id,
            "authority_gateway_contract": normalize_evm_address(&market.authority_gateway_contract),
            "evidence_rpc_sources": market.evidence_rpc_urls.len(),
        }))
    }

    fn describe_protected_content_creator_mint_source(&self) -> Response {
        let (network, mint) = match self.configured_global_protected_content_creator_mint_source() {
            Ok(source) => source,
            Err(response) => return response,
        };
        let Some(chain_id) = network.chain_id else {
            return Response::error(
                "protected_content_creator_mint_not_configured",
                "configured protected-content creator mint network is missing chain id",
            );
        };
        // The authority gateway this network's market is configured with, so a
        // mint can state it in its own metadata rather than leaving readers to
        // resolve it from whatever the runtime happens to have registered
        // later. `null` when the network has no market configured at all.
        let authority = self
            .configured_protected_content_market_source(&network.id)
            .ok()
            .map(|market| normalize_evm_address(&market.authority_gateway_contract));
        Response::ok(json!({
            "schema": PROTECTED_CONTENT_CREATOR_MINT_SOURCE_SCHEMA,
            "network": network.id,
            "chain_namespace": format!("eip155:{chain_id}"),
            // No channel: a mint settles on the one its creator chose, and a
            // source that named one here was read as "the" channel for years.
            // What the source still describes is the deployment -- which
            // contracts, which ABI, which market -- and what a creator may
            // price in.
            "pay_tokens": protected_content_pay_token_choices(mint)
                .into_iter()
                .zip(protected_content_pay_token_symbols(mint))
                .map(|(token, symbol)| json!({
                    "symbol": symbol,
                    "address": token.address,
                    "decimals": token.decimals,
                }))
                .collect::<Vec<_>>(),
            "abi": mint.abi,
            "function": mint.abi.function(),
            "authority_gateway_contract": authority,
        }))
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "creator mint assembly binds the exact on-chain mint terms without an intermediate authority struct"
    )]
    fn resolve_protected_content_creator_mint(
        &self,
        creator: &str,
        token_uri: &str,
        content_access_id: &str,
        copies: &str,
        price: &str,
        royalties: &[ProtectedContentRoyaltyShare],
        op_type_code: u16,
        reseller_cut: Option<u16>,
        ledger: Option<&str>,
        pay_token: Option<&str>,
    ) -> Response {
        let Some(method) = ProtectedContentAccessMethod::from_code(op_type_code) else {
            return Response::error(
                "invalid_protected_content_creator_mint_request",
                "op_type_code must be 0 (free), 1 (buy once) or 2 (buy and resell)",
            );
        };
        // The cut belongs to exactly one method. Accepting it elsewhere would
        // silently drop it, and omitting it on resale would shift the ABI
        // layout the factory decodes.
        let reseller_cut = match (method, reseller_cut) {
            (ProtectedContentAccessMethod::BuyAndResell, Some(cut)) => {
                if cut > PROTECTED_CONTENT_RESELLER_CUT_MAX {
                    return Response::error(
                        "invalid_protected_content_creator_mint_request",
                        "reseller_cut must not exceed the whole sale",
                    );
                }
                Some(cut)
            }
            (ProtectedContentAccessMethod::BuyAndResell, None) => {
                return Response::error(
                    "invalid_protected_content_creator_mint_request",
                    "buy and resell requires reseller_cut",
                )
            }
            (_, Some(_)) => {
                return Response::error(
                    "invalid_protected_content_creator_mint_request",
                    "reseller_cut applies only to buy and resell",
                )
            }
            (_, None) => None,
        };
        let (network, mint) = match self.configured_global_protected_content_creator_mint_source() {
            Ok(source) => source,
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_address(creator) {
            return Response::error("invalid_protected_content_creator_mint_request", &err);
        }
        // The channel a mint settles on is the creator's to choose, and a mint
        // without one is not a mint this Runtime will assemble: defaulting it
        // would publish into a channel nobody picked.
        let ledger = match ledger.map(str::trim).filter(|value| !value.is_empty()) {
            Some(ledger) => match validate_evm_address(ledger) {
                Ok(()) => normalize_evm_address(ledger),
                Err(err) => {
                    return Response::error("invalid_protected_content_creator_mint_request", &err)
                }
            },
            None => {
                return Response::error(
                    "invalid_protected_content_creator_mint_request",
                    "a channel must be chosen before a mint can be assembled",
                )
            }
        };
        let pay_token = match resolve_protected_content_pay_token(mint, pay_token) {
            Ok(pay_token) => pay_token,
            Err(err) => {
                return Response::error("invalid_protected_content_creator_mint_request", &err)
            }
        };

        if token_uri.trim().is_empty() {
            return Response::error(
                "invalid_protected_content_creator_mint_request",
                "token_uri must not be empty",
            );
        }
        let content_access_id = match decode_hex(content_access_id, Some(16), "content_access_id") {
            Ok(bytes) => match <[u8; 16]>::try_from(bytes.as_slice()) {
                Ok(bytes) => bytes,
                Err(_) => {
                    return Response::error(
                        "invalid_protected_content_creator_mint_request",
                        "content_access_id must be 16 bytes",
                    )
                }
            },
            Err(err) => {
                return Response::error("invalid_protected_content_creator_mint_request", &err);
            }
        };
        let copies = match normalize_hex_quantity(copies, "copies") {
            Ok(value) => value,
            Err(err) => {
                return Response::error("invalid_protected_content_creator_mint_request", &err);
            }
        };
        // A free mint creates no access token, so a supply is not a thing it
        // can express. A paid one must grant at least one.
        if method.is_paid() && copies == "0x0" {
            return Response::error(
                "invalid_protected_content_creator_mint_request",
                "copies must be greater than zero",
            );
        }
        let price = match normalize_hex_quantity(price, "price") {
            Ok(value) => value,
            Err(err) => {
                return Response::error("invalid_protected_content_creator_mint_request", &err);
            }
        };
        if method.is_paid() {
            if price == "0x0" {
                return Response::error(
                    "invalid_protected_content_creator_mint_request",
                    "price must be greater than zero",
                );
            }
        } else if price != "0x0" {
            // Refused rather than ignored. A free mint encodes no sell terms
            // at all, so a price accepted here would be a price the chain
            // never sees -- a listing claiming terms nobody applied.
            return Response::error(
                "invalid_protected_content_creator_mint_request",
                "a free mint must not carry a price",
            );
        }
        let creator = normalize_evm_address(creator);
        // A free mint carries the content id and nothing else: with no
        // operative there is no access token to mint, no royalty share to
        // split and no listing to create, so the payee arrays and the sell
        // terms have nowhere to land.
        let (op_raw_bytes, sell_raw_bytes) = if method.is_paid() {
            // The access token always mints to the creator. The royalty share
            // is split across the payees the creator named, or goes wholly to
            // them when they named none.
            let (mut addresses, mut roles, mut amounts) = (
                vec![creator.clone()],
                vec![PROTECTED_CONTENT_CREATOR_ACCESS_TOKEN_ROLE],
                vec![copies.clone()],
            );
            match protected_content_royalty_units(royalties, &creator) {
                Ok(shares) => {
                    for (address, units) in shares {
                        addresses.push(address);
                        roles.push(PROTECTED_CONTENT_CREATOR_ROYALTY_SHARE_ROLE);
                        amounts.push(units);
                    }
                }
                Err(err) => {
                    return Response::error("invalid_protected_content_creator_mint_request", &err);
                }
            }
            let op_raw = match encode_protected_content_mint_op_raw_paid(
                &content_access_id,
                token_uri,
                &addresses,
                &roles,
                &amounts,
                reseller_cut,
            ) {
                Ok(value) => value,
                Err(err) => {
                    return Response::error("invalid_protected_content_creator_mint_request", &err);
                }
            };
            let sell_raw =
                match encode_protected_content_sell_raw_data(&copies, &price, &pay_token.address) {
                    Ok(value) => value,
                    Err(err) => {
                        return Response::error(
                            "invalid_protected_content_creator_mint_request",
                            &err,
                        );
                    }
                };
            (op_raw, sell_raw)
        } else {
            if !royalties.is_empty() {
                return Response::error(
                    "invalid_protected_content_creator_mint_request",
                    "a free mint has no royalty share to split",
                );
            }
            match encode_protected_content_mint_op_raw_free(&content_access_id) {
                Ok(value) => (value, Vec::new()),
                Err(err) => {
                    return Response::error("invalid_protected_content_creator_mint_request", &err);
                }
            }
        };

        let data = match encode_protected_content_creator_mint_call(
            mint.abi.selector(),
            &protected_content_media_token_uri(token_uri),
            method.code(),
            &op_raw_bytes,
            &sell_raw_bytes,
        ) {
            Ok(data) => data,
            Err(err) => {
                return Response::error("invalid_protected_content_creator_mint_request", &err)
            }
        };
        let Some(chain_id) = network.chain_id else {
            return Response::error(
                "protected_content_creator_mint_not_configured",
                "configured protected-content creator mint network is missing chain id",
            );
        };

        let mut answer = json!({
            "schema": PROTECTED_CONTENT_CREATOR_MINT_SCHEMA,
            "network": network.id,
            "chain_namespace": format!("eip155:{chain_id}"),
            "function": mint.abi.function(),
            "ledger": ledger,
            "pay_token": pay_token.address,
            "pay_token_decimals": pay_token.decimals,
            "to": ledger,
            "data": data,
            "value": "0x0",
            "content_access_id": format!("0x{}", encode_hex(&content_access_id)),
            "op_type_code": method.code(),
            "signed": false,
        });
        // Stated only by the one method it belongs to. A null here would read
        // as "resale at no cut" rather than "resale is not on offer".
        if let Some(cut) = reseller_cut {
            answer["reseller_cut"] = json!(cut);
        }
        Response::ok(answer)
    }

    fn resolve_protected_content_mint_receipt(
        &self,
        network_id: &str,
        hash: &str,
        creator: &str,
        ledger: &str,
        token_uri: &str,
        op_type_code: u16,
    ) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        let mint = match self.configured_protected_content_creator_mint_source(network_id) {
            Ok(mint) => mint,
            Err(response) => return response,
        };
        let market = match self.configured_protected_content_market_source(network_id) {
            Ok(market) => market,
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_hash(hash) {
            return Response::error("invalid_protected_content_mint_receipt_request", &err);
        }
        if let Err(err) = validate_evm_address(creator) {
            return Response::error("invalid_protected_content_mint_receipt_request", &err);
        }
        if let Err(err) = validate_evm_address(ledger) {
            return Response::error("invalid_protected_content_mint_receipt_request", &err);
        }
        if token_uri.trim().is_empty() {
            return Response::error(
                "invalid_protected_content_mint_receipt_request",
                "token_uri must not be empty",
            );
        }
        let observation = match self.observe_protected_content_mint_receipt(
            network,
            market,
            mint,
            hash,
            creator,
            ledger,
            token_uri,
            op_type_code,
        ) {
            Ok(observation) => observation,
            Err(response) => return response,
        };
        Response::ok(json!({
            "schema": PROTECTED_CONTENT_MINT_RECEIPT_SCHEMA,
            "network": network.id,
            "chain_id": observation.chain_id,
            "token_id": observation.token_id,
            "operative": observation.operative,
            // The listing the mint emitted, so the caller does not read back
            // what this receipt already proves. Null throughout for a free
            // mint, which lists nothing -- stated as absent rather than as a
            // zero price, which a reader could mistake for a sale at no cost.
            "quantity": observation.listing.as_ref().map(|listing| &listing.quantity),
            "price": observation.listing.as_ref().map(|listing| &listing.price),
            "pay_token": observation.listing.as_ref().map(|listing| &listing.pay_token),
        }))
    }

    /// What this account already holds on each channel, so the Shops surface
    /// knows which cards to offer a subscription for.
    ///
    /// Read at the head rather than at a finalized block: this decides what a
    /// card OFFERS, and offering a subscription to someone who just bought one
    /// is a worse answer than reading state a few blocks young. Nothing here
    /// grants anything -- `subscribePlan` and every access check are decided by
    /// the chain when they happen.
    ///
    /// A channel whose sources disagree or cannot be read is reported as
    /// `unknown` rather than as `none`, and it is reported per channel: one
    /// unreadable channel must not take the whole shelf down, and it must not
    /// quietly read as "you have no access", which is the answer that would
    /// put a Subscribe button in front of someone who already subscribed.
    fn resolve_protected_content_channel_access(
        &self,
        network_id: &str,
        account: &str,
        channels: &[String],
    ) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network.clone(),
            Err(response) => return response,
        };
        let market = match self.configured_protected_content_market_source(network_id) {
            Ok(market) => market.clone(),
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_address(account) {
            return Response::error("invalid_protected_content_channel_access_request", &err);
        }
        if channels.len() > PROTECTED_CONTENT_CHANNEL_ACCESS_MAX {
            return Response::error(
                "invalid_protected_content_channel_access_request",
                "too many channels in one channel-access request",
            );
        }
        let subscription_data = match encode_channel_has_active_subscription_call(account) {
            Ok(data) => data,
            Err(err) => {
                return Response::error("invalid_protected_content_channel_access_request", &err)
            }
        };
        let admin_data = match encode_channel_has_admin_role_call(account) {
            Ok(data) => data,
            Err(err) => {
                return Response::error("invalid_protected_content_channel_access_request", &err)
            }
        };
        // Every question for every channel, asked of each source in ONE batch.
        // Two calls per channel sent singly is eighty-odd round trips for a
        // directory of fourteen, which measured 46 seconds on an installed
        // Home with every card showing "checking" throughout.
        let addressable: Vec<String> = channels
            .iter()
            .filter(|channel| validate_evm_address(channel).is_ok())
            .map(|channel| normalize_evm_address(channel))
            .collect();
        let mut calls = Vec::with_capacity(addressable.len() * 2);
        for channel in &addressable {
            calls.push((
                "eth_call".to_string(),
                json!([{ "to": channel, "data": subscription_data }, "latest"]),
            ));
            calls.push((
                "eth_call".to_string(),
                json!([{ "to": channel, "data": admin_data }, "latest"]),
            ));
        }
        // One vector of answers per source, so each channel's two questions can
        // be corroborated across sources exactly as a single read was.
        let per_source = self.protected_content_channel_answers(&network, &market, &calls);
        let mut entries = Vec::with_capacity(addressable.len());
        for (index, channel) in addressable.iter().enumerate() {
            let subscribed = corroborated_bool(&per_source, index * 2);
            let administrator = corroborated_bool(&per_source, index * 2 + 1);
            entries.push(json!({
                "channel": channel,
                "state": match (administrator, subscribed) {
                    // An administrator is told so even when they also hold a
                    // subscription: it is the more useful thing to know, and
                    // neither offers them anything to buy.
                    (Some(true), _) => "administrator",
                    (_, Some(true)) => "subscribed",
                    (Some(false), Some(false)) => "none",
                    _ => "unknown",
                },
            }));
        }
        Response::ok(json!({
            "schema": "elastos.chain.protected-content-channel-access/v1",
            "network": network.id,
            "account": normalize_evm_address(account),
            "channels": entries,
        }))
    }

    /// Every call, asked of every source, in chunks small enough to be
    /// accepted and run at the same time rather than one after another.
    ///
    /// Chunking alone is not enough: forty calls become fourteen batches per
    /// source and forty-two requests in all, which sequentially is still the
    /// better part of a minute with every card showing "checking". They do not
    /// depend on each other, so they are asked together.
    fn protected_content_channel_answers(
        &self,
        network: &ChainNetwork,
        market: &ProtectedContentMarketMethod,
        calls: &[(String, Value)],
    ) -> Vec<Vec<Option<bool>>> {
        let chunks: Vec<&[(String, Value)]> = calls.chunks(EVM_RPC_BATCH_MAX).collect();
        let sources = &market.evidence_rpc_urls;
        // Every (source, chunk) pair, and where its answers belong.
        let tasks: Vec<(usize, usize)> = (0..sources.len())
            .flat_map(|source| (0..chunks.len()).map(move |chunk| (source, chunk)))
            .collect();
        let next = std::sync::Mutex::new(0usize);
        let results: std::sync::Mutex<Vec<Vec<Option<bool>>>> =
            std::sync::Mutex::new(vec![vec![None; calls.len()]; sources.len()]);
        let workers = tasks.len().min(PROTECTED_CONTENT_CHANNEL_ACCESS_WORKERS);
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| loop {
                    let task = {
                        let mut guard = next.lock().unwrap_or_else(|err| err.into_inner());
                        let index = *guard;
                        if index >= tasks.len() {
                            return;
                        }
                        *guard += 1;
                        tasks[index]
                    };
                    let (source, chunk) = task;
                    let mut source_network = network.clone();
                    source_network.rpc_url = sources[source].to_string();
                    let calls = chunks[chunk];
                    let answers = match self.evm_rpc_batch(&source_network, calls) {
                        Ok(answers) => answers,
                        // A source that will not answer a batch at all is asked
                        // singly rather than dropped: sources refusing batches
                        // would otherwise leave every card permanently unknown.
                        Err(_) => self.protected_content_channel_calls_serially(&source_network, calls),
                    };
                    let offset = chunk * EVM_RPC_BATCH_MAX;
                    let mut guard = results.lock().unwrap_or_else(|err| err.into_inner());
                    for (index, answer) in answers.into_iter().enumerate() {
                        guard[source][offset + index] =
                            answer.and_then(|value| decode_evm_bool(&value).ok());
                    }
                });
            }
        });
        results.into_inner().unwrap_or_else(|err| err.into_inner())
    }

    /// The same calls, one request each, for a source that refused a batch.
    fn protected_content_channel_calls_serially(
        &self,
        network: &ChainNetwork,
        calls: &[(String, Value)],
    ) -> Vec<Option<Value>> {
        calls
            .iter()
            .map(|(method, params)| self.evm_rpc(network, method, params.clone()).ok())
            .collect()
    }

    /// The payment processor an operative pays an ERC-20 sale through.
    ///
    /// Read at the head rather than at a finalized block, which is the whole
    /// point of it being separate. A mint's own receipt already proves the
    /// sale terms -- the `ItemListed` is in the very transaction that settled
    /// -- so re-proving them at finality made a listing wait out L1 finality
    /// for one address that the event does not carry. That address belongs to
    /// the operative the receipt already named, and a buyer's own purchase
    /// re-reads and re-checks it before any money moves.
    ///
    /// Corroborated across sources all the same: one endpoint's answer about
    /// where a payment goes is not evidence.
    fn resolve_protected_content_payment_processor(
        &self,
        network_id: &str,
        operative: &str,
    ) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network.clone(),
            Err(response) => return response,
        };
        let market = match self.configured_protected_content_market_source(network_id) {
            Ok(market) => market.clone(),
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_address(operative) {
            return Response::error("invalid_protected_content_payment_processor_request", &err);
        }
        let data = match encode_operatives_payment_processor_call() {
            Ok(data) => data,
            Err(err) => {
                return Response::error("invalid_protected_content_payment_processor_request", &err)
            }
        };
        let mut answers = Vec::new();
        for rpc_url in &market.evidence_rpc_urls {
            let mut source_network = network.clone();
            source_network.rpc_url = rpc_url.to_string();
            let Ok(result) = self.evm_rpc(
                &source_network,
                "eth_call",
                json!([{ "to": operative, "data": data }, "latest"]),
            ) else {
                continue;
            };
            let Ok(address) = decode_evm_address_word(&result, "payment_processor") else {
                continue;
            };
            answers.push(normalize_evm_address(&address));
        }
        if answers.len() < 2 {
            return Response::error(
                "protected_content_payment_processor_pending",
                "protected-content payment processor is not confirmed on enough configured sources",
            );
        }
        if answers[1..].iter().any(|answer| answer != &answers[0]) {
            return Response::error(
                "conflicting_protected_content_payment_processor_observations",
                "protected-content payment processor sources disagree",
            );
        }
        Response::ok(json!({
            "schema": "elastos.chain.protected-content-payment-processor/v1",
            "network": network.id,
            "operative": normalize_evm_address(operative),
            "payment_processor": answers[0],
        }))
    }

    fn resolve_protected_content_verified_listing(
        &self,
        network_id: &str,
        seller: &str,
        ledger: &str,
        token_id: &str,
    ) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        if let Err(err) = validate_evm_address(seller) {
            return Response::error("invalid_protected_content_purchase_request", &err);
        }
        if let Err(err) = validate_evm_address(ledger) {
            return Response::error("invalid_protected_content_purchase_request", &err);
        }
        if let Err(err) = validate_hex_quantity(token_id, "token_id") {
            return Response::error("invalid_protected_content_purchase_request", &err);
        }
        let seller = normalize_evm_address(seller);
        let ledger = normalize_evm_address(ledger);
        let token_id = match normalize_hex_quantity(token_id, "token_id") {
            Ok(value) => value,
            Err(err) => return Response::error("invalid_protected_content_purchase_request", &err),
        };
        let market = match self.configured_protected_content_market_source(network_id) {
            Ok(market) => market,
            Err(response) => return response,
        };
        let verified_listing = match self.observe_protected_content_verified_listing(
            network, market, &seller, &ledger, &token_id,
        ) {
            Ok(verified_listing) => verified_listing,
            Err(response) => return response,
        };
        let mut data = json!({
            "schema": PROTECTED_CONTENT_VERIFIED_LISTING_SCHEMA,
            "network": network.id,
            "chain_id": verified_listing.chain_id,
            "seller": seller,
            "ledger": ledger,
            "token_id": token_id,
            "operative": verified_listing.operative,
            "quantity": verified_listing.quantity,
            "price": verified_listing.price,
            "pay_token": verified_listing.pay_token,
        });
        if let Some(payment_processor) = verified_listing.payment_processor {
            data["payment_processor"] = json!(payment_processor);
        }
        Response::ok(data)
    }

    fn resolve_protected_content_purchase(
        &self,
        seller: &str,
        chain_namespace: &str,
        network_id: &str,
        ledger: &str,
        token_id: &str,
    ) -> Response {
        let network = match self.evm_network(network_id) {
            Ok(network) => network,
            Err(response) => return response,
        };
        let Some(expected_chain_id) = network.chain_id else {
            return Response::error(
                "protected_content_market_not_configured",
                "no configured protected-content market source matches listing network",
            );
        };
        if chain_namespace != format!("eip155:{expected_chain_id}") {
            return Response::error(
                "invalid_protected_content_purchase_request",
                "protected-content purchase request Chain namespace does not match network",
            );
        }
        if let Err(err) = validate_evm_address(seller) {
            return Response::error("invalid_protected_content_purchase_request", &err);
        }
        if let Err(err) = validate_evm_address(ledger) {
            return Response::error("invalid_protected_content_purchase_request", &err);
        }
        if let Err(err) = validate_hex_quantity(token_id, "token_id") {
            return Response::error("invalid_protected_content_purchase_request", &err);
        }
        let seller = normalize_evm_address(seller);
        let ledger = normalize_evm_address(ledger);
        let token_id = match normalize_hex_quantity(token_id, "token_id") {
            Ok(value) => value,
            Err(err) => return Response::error("invalid_protected_content_purchase_request", &err),
        };
        let market = match self.configured_protected_content_market_source(network_id) {
            Ok(market) => market,
            Err(response) => return response,
        };
        let verified_listing = match self.observe_protected_content_verified_listing(
            network, market, &seller, &ledger, &token_id,
        ) {
            Ok(verified_listing) => verified_listing,
            Err(response) => return response,
        };
        if verified_listing.quantity == "0x0" {
            return Response::error(
                "protected_content_verified_listing_unavailable",
                "verified listing does not have available stock for a one-copy purchase",
            );
        }
        let market_contract = normalize_evm_address(&market.authority_gateway_contract);
        let price = verified_listing.price.clone();
        let verified_pay_token = verified_listing.pay_token.clone();
        let is_native_purchase = verified_pay_token == "0x0000000000000000000000000000000000000000";
        let buy_call = match encode_authority_gateway_buy_access_call(
            if is_native_purchase {
                PROTECTED_CONTENT_BUY_ACCESS_NATIVE_SELECTOR
            } else {
                PROTECTED_CONTENT_BUY_ACCESS_ERC20_SELECTOR
            },
            &seller,
            &ledger,
            &token_id,
            PROTECTED_CONTENT_PURCHASE_QUANTITY_HEX,
            &price,
            (!is_native_purchase).then_some(verified_pay_token.as_str()),
        ) {
            Ok(data) => data,
            Err(err) => return Response::error("invalid_protected_content_purchase_request", &err),
        };
        let steps = if is_native_purchase {
            vec![json!({
                "stage": "buy",
                "to": market_contract,
                "value": price,
                "data": buy_call,
            })]
        } else {
            let Some(payment_processor) = verified_listing.payment_processor.as_deref() else {
                return Response::error(
                    "upstream_invalid_protected_content_verified_listing",
                    "ERC-20 verified listing is missing paymentProcessor",
                );
            };
            let approval_call = match encode_erc20_approve_call(payment_processor, &price) {
                Ok(data) => data,
                Err(err) => {
                    return Response::error("invalid_protected_content_purchase_request", &err)
                }
            };
            vec![
                json!({
                    "stage": "approval",
                    "to": verified_pay_token,
                    "value": "0x0",
                    "data": approval_call,
                }),
                json!({
                    "stage": "buy",
                    "to": market_contract,
                    "value": "0x0",
                    "data": buy_call,
                }),
            ]
        };
        Response::ok(json!({
            "schema": PROTECTED_CONTENT_PURCHASE_SCHEMA,
            "network": network.id,
            "purchase_quantity": PROTECTED_CONTENT_PURCHASE_QUANTITY_HEX,
            "verified_listing": {
                "chain_id": verified_listing.chain_id,
                "seller": seller,
                "ledger": ledger,
                "token_id": token_id,
                "operative": verified_listing.operative,
                "available_quantity": verified_listing.quantity,
                "price": verified_listing.price,
                "pay_token": verified_listing.pay_token,
                "payment_processor": verified_listing.payment_processor,
            },
            "steps": steps,
        }))
    }

    fn resolve_protected_content_purchase_access(
        &self,
        request_id: &str,
        network_id: &str,
        wallet: &str,
        content_access_id: &str,
    ) -> Response {
        if request_id.trim().is_empty() || request_id.len() > 256 {
            return Response::error(
                "invalid_protected_content_purchase_access_request",
                "protected-content purchase access request identity is invalid",
            );
        }
        if let Err(err) = validate_evm_address(wallet) {
            return Response::error("invalid_protected_content_purchase_access_request", &err);
        }
        let content_access_id = match parse_content_access_id(content_access_id) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let (network, method, policy_source) = match self
            .configured_protected_content_policy_source_for_network(
                network_id,
                ProtectedContentPolicyAction::View,
            ) {
            Ok(source) => source,
            Err(response) => return response,
        };
        let Some(expected_chain_id) = network.chain_id else {
            return Response::error(
                "protected_content_purchase_access_not_configured",
                "no configured protected-content purchase access source matches listing network",
            );
        };
        let data = match method.abi {
            RightsMethodAbi::HasAccessByContentIdAddressBytes16 => {
                match encode_has_access_by_content_id_call(
                    &method.selector,
                    content_access_id.as_bytes(),
                    &normalize_evm_address(wallet),
                ) {
                    Ok(data) => data,
                    Err(err) => {
                        return Response::error(
                            "invalid_protected_content_purchase_access_request",
                            &err,
                        )
                    }
                }
            }
        };
        let observation = match self.observe_protected_content_rights(
            network,
            &policy_source.evidence_rpc_urls,
            expected_chain_id,
            ProtectedContentRightsCall {
                contract: &method.contract,
                data: &data,
                expected_content_access_id: &content_access_id,
                // The upfront "is this copy theirs" check. The grant is
                // readable at the block that carries the acquisition, so
                // reading it at finality would deny a transaction the caller
                // watched confirm. Releasing the key is gated separately, on
                // finalized evidence.
                block: ProtectedContentRightsBlock::Head,
            },
        ) {
            Ok(observation) => observation,
            Err(response) => return response,
        };
        let has_access = match observation.outcome {
            ProtectedContentRightsObservationKind::HasAccess(has_access) => has_access,
            ProtectedContentRightsObservationKind::Unbound(_) => {
                return Response::error(
                    "unknown_protected_content_object",
                    "protected-content content access id is not bound on chain",
                )
            }
        };
        let now = (self.now_unix_seconds)();
        if let Err(err) = validate_finalized_observation_freshness(
            observation.finalized_block_timestamp,
            now,
            PROTECTED_CONTENT_PURCHASE_ACCESS_MAX_FINALIZED_AGE_SECS,
            PROTECTED_CONTENT_PURCHASE_ACCESS_MAX_FUTURE_SKEW_SECS,
        ) {
            return Response::error("stale_protected_content_purchase_access_observation", &err);
        }
        Response::ok(json!({
            "schema": PROTECTED_CONTENT_PURCHASE_ACCESS_SCHEMA,
            "request_id": request_id,
            "network": network.id,
            "chain_id": observation.chain_id,
            "wallet": normalize_evm_address(wallet),
            "content_access_id": format!("0x{}", encode_hex(content_access_id.as_bytes())),
            "has_access": has_access,
            "finalized_block_number": observation.finalized_block_number,
            "finalized_block_hash": format!("0x{}", encode_hex(observation.finalized_block_hash.as_bytes())),
            "finalized_block_timestamp": observation.finalized_block_timestamp,
            "observed_at": now,
        }))
    }

    fn configured_global_protected_content_creator_mint_source(
        &self,
    ) -> Result<(&ChainNetwork, &ProtectedContentCreatorMintMethod), Response> {
        let mut matches = self
            .networks
            .iter()
            .filter(|network| network.kind == ChainKind::EvmJsonRpc)
            .filter_map(|network| {
                network
                    .protected_content_creator_mint
                    .as_ref()
                    .map(|mint| (network, mint))
            });
        let Some(source) = matches.next() else {
            return Err(Response::error(
                "protected_content_creator_mint_not_configured",
                "no configured protected-content creator mint source is available",
            ));
        };
        if matches.next().is_some() {
            return Err(Response::error(
                "ambiguous_protected_content_creator_mint_source",
                "multiple protected-content creator mint sources are configured",
            ));
        }
        Ok(source)
    }

    fn configured_protected_content_creator_mint_source(
        &self,
        network_id: &str,
    ) -> Result<&ProtectedContentCreatorMintMethod, Response> {
        let mut matches = self
            .networks
            .iter()
            .filter(|network| network.kind == ChainKind::EvmJsonRpc && network.id == network_id)
            .filter_map(|network| network.protected_content_creator_mint.as_ref());
        let Some(source) = matches.next() else {
            return Err(Response::error(
                "protected_content_creator_mint_not_configured",
                "no configured protected-content creator mint source matches network",
            ));
        };
        if matches.next().is_some() {
            return Err(Response::error(
                "ambiguous_protected_content_creator_mint_source",
                "multiple protected-content creator mint sources match network",
            ));
        }
        Ok(source)
    }

    fn configured_protected_content_policy_source_for_policy(
        &self,
        policy: &RightsPolicyBodyV1,
    ) -> Result<(&ChainNetwork, &RightsMethod, &ProtectedContentPolicySource), Response> {
        let contract = format!("0x{}", encode_hex(policy.contract_address().as_bytes()));
        let selector = format!("0x{}", encode_hex(policy.function_selector().as_bytes()));
        let required_action = policy.required_action();
        let mut matches = self
            .networks
            .iter()
            .filter(|network| {
                network.kind == ChainKind::EvmJsonRpc && network.chain_id == Some(policy.chain_id())
            })
            .flat_map(|network| {
                let contract = contract.clone();
                let selector = selector.clone();
                network.rights_methods.iter().flat_map(move |method| {
                    let contract = contract.clone();
                    let selector = selector.clone();
                    method
                        .protected_content_policies
                        .iter()
                        .filter(move |policy_source| {
                            method.id == "has_access_by_content_id"
                                && method.contract.eq_ignore_ascii_case(&contract)
                                && method.selector == selector
                                && policy_source.action.to_contract_action() == required_action
                        })
                        .map(move |policy_source| (network, method, policy_source))
                })
            });
        let Some(source) = matches.next() else {
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
        Ok(source)
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

    fn configured_protected_content_policy_source_for_network(
        &self,
        network_id: &str,
        action: ProtectedContentPolicyAction,
    ) -> Result<(&ChainNetwork, &RightsMethod, &ProtectedContentPolicySource), Response> {
        let mut matches = self
            .networks
            .iter()
            .filter(|network| network.kind == ChainKind::EvmJsonRpc && network.id == network_id)
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
                "protected_content_purchase_access_not_configured",
                "no configured protected-content purchase access source matches listing network",
            ));
        };
        if matches.next().is_some() {
            return Err(Response::error(
                "ambiguous_protected_content_purchase_access_source",
                "multiple protected-content purchase access sources match listing network",
            ));
        }
        Ok(source)
    }

    fn configured_protected_content_market_source(
        &self,
        network_id: &str,
    ) -> Result<&ProtectedContentMarketMethod, Response> {
        let mut matches = self
            .networks
            .iter()
            .filter(|network| network.kind == ChainKind::EvmJsonRpc && network.id == network_id)
            .filter_map(|network| network.protected_content_market.as_ref());
        let Some(source) = matches.next() else {
            return Err(Response::error(
                "protected_content_market_not_configured",
                "no configured protected-content market source matches listing network",
            ));
        };
        if matches.next().is_some() {
            return Err(Response::error(
                "ambiguous_protected_content_market_source",
                "multiple protected-content market sources match listing network",
            ));
        }
        Ok(source)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "mint receipt corroboration binds one exact transaction receipt to creator, ledger, token URI, and op type"
    )]
    fn observe_protected_content_mint_receipt(
        &self,
        network: &ChainNetwork,
        market: &ProtectedContentMarketMethod,
        mint: &ProtectedContentCreatorMintMethod,
        hash: &str,
        creator: &str,
        ledger: &str,
        token_uri: &str,
        op_type_code: u16,
    ) -> Result<ProtectedContentMintReceiptObservation, Response> {
        let mut successful = Vec::new();
        let mut first_error = None;
        for rpc_url in &market.evidence_rpc_urls {
            match self.observe_protected_content_mint_receipt_source(
                network,
                rpc_url,
                mint,
                market,
                hash,
                creator,
                ledger,
                token_uri,
                op_type_code,
            ) {
                Ok(Some(observation)) => successful.push(observation),
                // This source has not reached the confirmation depth yet.
                Ok(None) => {}
                Err(response) => {
                    if first_error.is_none() {
                        first_error = Some(response);
                    }
                }
            }
        }
        if successful
            .iter()
            .any(|observation| observation.chain_id != network.chain_id.unwrap_or_default())
        {
            return Err(Response::error(
                "conflicting_protected_content_mint_receipt_observations",
                "protected-content mint receipt sources disagree with configured chain id",
            ));
        }
        if successful.len() >= 2 {
            let reference = successful[0].clone();
            if successful[1..]
                .iter()
                .any(|observation| *observation != reference)
            {
                return Err(Response::error(
                    "conflicting_protected_content_mint_receipt_observations",
                    "protected-content mint receipt sources disagree on the exact finalized bind",
                ));
            }
            return Ok(reference);
        }
        // A source that refused outright is the only thing worth reporting as a
        // failure: it says something about this receipt that waiting will not
        // change.
        if let Some(response) = first_error {
            return Err(response);
        }
        // Otherwise the corroborating quorum simply has not caught up. Two
        // independent RPCs confirm a block at their own pace, so one source
        // ahead of the other is the ORDINARY shape of a fresh mint, not a
        // refusal -- and the transaction it describes may already be on chain.
        // Reporting it as anything but pending abandons a settled mint.
        Err(Response::error(
            "protected_content_mint_receipt_pending",
            "protected-content mint receipt is not yet confirmed on enough configured sources",
        ))
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "mint receipt corroboration binds one exact transaction receipt to creator, ledger, token URI, and op type"
    )]
    fn observe_protected_content_mint_receipt_source(
        &self,
        network: &ChainNetwork,
        rpc_url: &str,
        mint: &ProtectedContentCreatorMintMethod,
        market: &ProtectedContentMarketMethod,
        hash: &str,
        creator: &str,
        ledger: &str,
        token_uri: &str,
        op_type_code: u16,
    ) -> Result<Option<ProtectedContentMintReceiptObservation>, Response> {
        let mut source_network = network.clone();
        source_network.rpc_url = rpc_url.to_string();
        let chain_id = match self
            .evm_rpc(&source_network, "eth_chainId", json!([]))
            .ok()
            .and_then(|value| value.as_str().and_then(|value| parse_hex_u64(value).ok()))
        {
            Some(chain_id) => chain_id,
            None => return Ok(None),
        };
        let receipt = match self
            .evm_rpc(&source_network, "eth_getTransactionReceipt", json!([hash]))
            .ok()
        {
            Some(receipt) if !receipt.is_null() => receipt,
            Some(_) | None => return Ok(None),
        };
        let status = match receipt.get("status").and_then(Value::as_str) {
            Some(status) => status,
            None => {
                return Err(Response::error(
                    "invalid_protected_content_mint_receipt",
                    "protected-content mint receipt status is missing",
                ))
            }
        };
        if status == "0x0" {
            return Err(Response::error(
                "protected_content_mint_receipt_failed",
                "protected-content mint transaction failed",
            ));
        }
        if status != "0x1" {
            return Err(Response::error(
                "invalid_protected_content_mint_receipt",
                "protected-content mint receipt status must be exactly 0x1",
            ));
        }
        let Some(receipt_transaction_hash) = receipt.get("transactionHash").and_then(Value::as_str)
        else {
            return Err(Response::error(
                "invalid_protected_content_mint_receipt",
                "protected-content mint receipt transaction hash is missing",
            ));
        };
        if !receipt_transaction_hash.eq_ignore_ascii_case(hash) {
            return Err(Response::error(
                "invalid_protected_content_mint_receipt",
                "protected-content mint receipt transaction hash does not match request",
            ));
        }
        let Some(receipt_from) = receipt.get("from").and_then(Value::as_str) else {
            return Err(Response::error(
                "invalid_protected_content_mint_receipt",
                "protected-content mint receipt signer is missing",
            ));
        };
        if !receipt_from.eq_ignore_ascii_case(creator) {
            return Err(Response::error(
                "invalid_protected_content_mint_receipt",
                "protected-content mint receipt signer does not match creator",
            ));
        }
        // The transaction's own target is deliberately NOT checked against the
        // ledger. It names whichever contract the signer's account called
        // first, which is the ledger only for a plain EOA: an EIP-7702
        // delegated account -- now the default for a MetaMask user who has
        // upgraded -- sends to its delegation executor, which then calls the
        // ledger. Requiring `to == ledger` rejected mints that had in fact
        // settled, and would reject every smart-account creator.
        //
        // Nothing is given up by dropping it. The ledger binding is proven
        // below, and far more strongly: the loop accepts a log only from the
        // configured `asset_created_emitter`, and the decoded `AssetCreated`
        // must carry this exact ledger, this creator, this token URI and this
        // op type. `to` bound none of those. What the receipt still fixes here
        // is the signer (`from`), the transaction hash, and the status.
        let receipt_block_number = receipt
            .get("blockNumber")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Response::error(
                    "invalid_protected_content_mint_receipt",
                    "protected-content mint receipt block number is missing",
                )
            })
            .and_then(|value| {
                parse_hex_u64(value).map_err(|_| {
                    Response::error(
                        "invalid_protected_content_mint_receipt",
                        "protected-content mint receipt block number must be a hex quantity",
                    )
                })
            })?;
        let receipt_block_hash = receipt
            .get("blockHash")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Response::error(
                    "invalid_protected_content_mint_receipt",
                    "protected-content mint receipt block hash is missing",
                )
            })
            .and_then(|value| {
                decode_hex(value, Some(32), "protected-content mint receipt block hash").map_err(
                    |_| {
                        Response::error(
                            "invalid_protected_content_mint_receipt",
                            "protected-content mint receipt block hash must be 32 bytes",
                        )
                    },
                )
            })
            .and_then(|value| {
                let bytes: [u8; 32] = value.as_slice().try_into().map_err(|_| {
                    Response::error(
                        "invalid_protected_content_mint_receipt",
                        "protected-content mint receipt block hash must be 32 bytes",
                    )
                })?;
                Ok(Digest32::new(bytes))
            })?;
        let canonical_block = match self
            .evm_rpc(
                &source_network,
                "eth_getBlockByNumber",
                json!([format!("0x{receipt_block_number:x}"), false]),
            )
            .ok()
        {
            Some(block) if !block.is_null() => block,
            Some(_) | None => return Ok(None),
        };
        let canonical_number = canonical_block
            .get("number")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Response::error(
                    "invalid_protected_content_mint_receipt",
                    "canonical block number is missing",
                )
            })
            .and_then(|value| {
                parse_hex_u64(value).map_err(|_| {
                    Response::error(
                        "invalid_protected_content_mint_receipt",
                        "canonical block number must be a hex quantity",
                    )
                })
            })?;
        let canonical_hash = canonical_block
            .get("hash")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Response::error(
                    "invalid_protected_content_mint_receipt",
                    "canonical block hash is missing",
                )
            })
            .and_then(|value| {
                decode_hex(value, Some(32), "canonical block hash").map_err(|_| {
                    Response::error(
                        "invalid_protected_content_mint_receipt",
                        "canonical block hash must be 32 bytes",
                    )
                })
            })
            .and_then(|value| {
                let bytes: [u8; 32] = value.as_slice().try_into().map_err(|_| {
                    Response::error(
                        "invalid_protected_content_mint_receipt",
                        "canonical block hash must be 32 bytes",
                    )
                })?;
                Ok(Digest32::new(bytes))
            })?;
        if canonical_number != receipt_block_number || canonical_hash != receipt_block_hash {
            return Err(Response::error(
                "invalid_protected_content_mint_receipt",
                "protected-content mint receipt block does not match the canonical block",
            ));
        }
        let canonical_transactions = canonical_block
            .get("transactions")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                Response::error(
                    "invalid_protected_content_mint_receipt",
                    "canonical block transaction list is missing",
                )
            })?;
        if !canonical_transactions.iter().any(|entry| {
            entry
                .as_str()
                .is_some_and(|transaction_hash| transaction_hash.eq_ignore_ascii_case(hash))
        }) {
            return Err(Response::error(
                "invalid_protected_content_mint_receipt",
                "canonical block does not contain the protected-content mint transaction",
            ));
        }
        // Depth, not finality. The mint carries its own listing -- `ItemListed`
        // is emitted inside this very transaction -- so nothing is spent or
        // signed after this check; what it guards is a local record. Requiring
        // L1 finality made a creator wait twelve minutes for that record on a
        // good day, and far longer whenever Ethereum finality lagged, while
        // their asset was already live and already indexed.
        let head = match self
            .evm_rpc(&source_network, "eth_blockNumber", json!([]))
            .ok()
            .and_then(|value| value.as_str().and_then(|value| parse_hex_u64(value).ok()))
        {
            Some(head) => head,
            None => return Ok(None),
        };
        let confirmations = head.saturating_sub(receipt_block_number).saturating_add(1);
        if confirmations < mint.mint_confirmations {
            return Ok(None);
        }
        let logs = match receipt.get("logs").and_then(Value::as_array) {
            Some(logs) => logs,
            None => {
                return Err(Response::error(
                    "invalid_protected_content_mint_receipt",
                    "protected-content mint receipt logs are missing",
                ))
            }
        };
        let mut matches = Vec::new();
        for log in logs {
            let Some(address) = log.get("address").and_then(Value::as_str) else {
                continue;
            };
            if !address.eq_ignore_ascii_case(&mint.asset_created_emitter) {
                continue;
            }
            let Some(topic0) = log
                .get("topics")
                .and_then(Value::as_array)
                .and_then(|topics| topics.first())
                .and_then(Value::as_str)
            else {
                return Err(Response::error(
                    "invalid_protected_content_mint_receipt",
                    "configured AssetCreated log is malformed",
                ));
            };
            if !topic0.eq_ignore_ascii_case(mint.abi.asset_created_topic0()) {
                continue;
            }
            let decoded = decode_protected_content_asset_created_log(log)
                .map_err(|err| Response::error("invalid_protected_content_mint_receipt", &err))?;
            if !decoded.creator.eq_ignore_ascii_case(creator) {
                return Err(Response::error(
                    "invalid_protected_content_mint_receipt",
                    "AssetCreated creator does not match expected creator",
                ));
            }
            if !decoded.ledger.eq_ignore_ascii_case(ledger) {
                return Err(Response::error(
                    "invalid_protected_content_mint_receipt",
                    "AssetCreated channel does not match expected ledger",
                ));
            }
            if decoded.token_uri != protected_content_media_token_uri(token_uri) {
                return Err(Response::error(
                    "invalid_protected_content_mint_receipt",
                    "AssetCreated token URI does not match expected token URI",
                ));
            }
            if decoded.op_type_code != op_type_code {
                return Err(Response::error(
                    "invalid_protected_content_mint_receipt",
                    "AssetCreated op type does not match expected mint op type",
                ));
            }
            matches.push(decoded);
        }
        match matches.as_slice() {
            [] => Err(Response::error(
                "protected_content_mint_receipt_not_bound",
                "receipt does not contain the configured AssetCreated bind for this mint",
            )),
            [decoded] => {
                // A free mint lists nothing. `AssetFactory` only reaches
                // `_listAccess` through the operative branch it skips for op
                // type 0, so there is no `ItemListed` in this receipt to find
                // and demanding one would refuse a mint that succeeded.
                let listing = if ProtectedContentAccessMethod::from_code(op_type_code)
                    .is_some_and(ProtectedContentAccessMethod::is_paid)
                {
                    Some(self.protected_content_item_listed_in_receipt(
                        logs,
                        market,
                        creator,
                        &decoded.operative,
                    )?)
                } else {
                    None
                };
                Ok(Some(ProtectedContentMintReceiptObservation {
                    chain_id,
                    receipt_block_number,
                    receipt_block_hash,
                    token_id: decoded.token_id.clone(),
                    operative: decoded.operative.clone(),
                    listing,
                }))
            }
            _ => Err(Response::error(
                "ambiguous_protected_content_mint_receipt",
                "receipt contains multiple matching AssetCreated binds",
            )),
        }
    }

    /// The listing this mint created, taken from its own receipt.
    ///
    /// Bound as tightly as the `AssetCreated` check above: the event must come
    /// from the configured trade gateway, and must name this creator as seller
    /// and the operative the mint just produced. A receipt that mints without
    /// listing is refused rather than guessed at -- since the v3 protocol
    /// bundled the two, a mint without an `ItemListed` is not the shape this
    /// Runtime knows how to record.
    fn protected_content_item_listed_in_receipt(
        &self,
        logs: &[Value],
        market: &ProtectedContentMarketMethod,
        creator: &str,
        operative: &str,
    ) -> Result<ProtectedContentListingRead, Response> {
        let mut matches = Vec::new();
        for log in logs {
            let Some(address) = log.get("address").and_then(Value::as_str) else {
                continue;
            };
            if !address.eq_ignore_ascii_case(&market.authority_gateway_contract) {
                continue;
            }
            let Some(topic0) = log
                .get("topics")
                .and_then(Value::as_array)
                .and_then(|topics| topics.first())
                .and_then(Value::as_str)
            else {
                continue;
            };
            if !topic0.eq_ignore_ascii_case(PROTECTED_CONTENT_ITEM_LISTED_TOPIC0) {
                continue;
            }
            let decoded = decode_protected_content_item_listed_log(log)
                .map_err(|err| Response::error("invalid_protected_content_mint_receipt", &err))?;
            if !decoded.seller.eq_ignore_ascii_case(creator) {
                continue;
            }
            if !decoded.operative.eq_ignore_ascii_case(operative) {
                continue;
            }
            matches.push(decoded.listing);
        }
        match matches.as_slice() {
            [] => Err(Response::error(
                "protected_content_mint_receipt_not_bound",
                "receipt does not contain the configured ItemListed bind for this mint",
            )),
            [listing] => Ok(listing.clone()),
            _ => Err(Response::error(
                "ambiguous_protected_content_mint_receipt",
                "receipt contains multiple matching ItemListed binds",
            )),
        }
    }

    fn observe_protected_content_verified_listing(
        &self,
        network: &ChainNetwork,
        market: &ProtectedContentMarketMethod,
        seller: &str,
        ledger: &str,
        token_id: &str,
    ) -> Result<ProtectedContentVerifiedListingObservation, Response> {
        let mut successful = Vec::new();
        let mut first_error = None;
        for rpc_url in &market.evidence_rpc_urls {
            match self.observe_protected_content_verified_listing_source(
                network, market, rpc_url, seller, ledger, token_id,
            ) {
                Ok(Some(observation)) => successful.push(observation),
                Ok(None) => {}
                Err(response) => {
                    if first_error.is_none() {
                        first_error = Some(response);
                    }
                }
            }
        }
        if successful
            .iter()
            .any(|observation| observation.chain_id != network.chain_id.unwrap_or_default())
        {
            return Err(Response::error(
                "conflicting_protected_content_verified_listing_observations",
                "protected-content verified listing sources disagree with configured chain id",
            ));
        }
        if successful.len() >= 2 {
            let reference = successful[0].clone();
            if successful[1..]
                .iter()
                .any(|observation| *observation != reference)
            {
                return Err(Response::error(
                    "conflicting_protected_content_verified_listing_observations",
                    "protected-content verified listing sources disagree on the finalized tuple",
                ));
            }
            return Ok(reference);
        }
        if successful.is_empty() {
            if let Some(response) = first_error {
                return Err(response);
            }
        }
        Err(Response::error(
            "insufficient_protected_content_verified_listing_observations",
            "protected-content verified listing sources produced fewer than two matching finalized tuples",
        ))
    }

    fn observe_protected_content_verified_listing_source(
        &self,
        network: &ChainNetwork,
        market: &ProtectedContentMarketMethod,
        rpc_url: &str,
        seller: &str,
        ledger: &str,
        token_id: &str,
    ) -> Result<Option<ProtectedContentVerifiedListingObservation>, Response> {
        let mut source_network = network.clone();
        source_network.rpc_url = rpc_url.to_string();
        let chain_id = match self
            .evm_rpc(&source_network, "eth_chainId", json!([]))
            .ok()
            .and_then(|value| value.as_str().and_then(|value| parse_hex_u64(value).ok()))
        {
            Some(chain_id) => chain_id,
            None => return Ok(None),
        };
        let finalized = match self
            .evm_rpc(
                &source_network,
                "eth_getBlockByNumber",
                json!(["finalized", false]),
            )
            .ok()
            .and_then(|value| evm_finalized_block(&value).ok())
        {
            Some(finalized) => finalized,
            None => return Ok(None),
        };
        let operative_data = encode_authority_gateway_operative_call(ledger, token_id)
            .map_err(|err| Response::error("invalid_protected_content_purchase_request", &err))?;
        let operative_result = match self
            .evm_rpc(
                &source_network,
                "eth_call",
                json!([
                    { "to": market.authority_gateway_contract, "data": operative_data },
                    {
                        "blockHash": format!("0x{}", encode_hex(finalized.finalized_block_hash.as_bytes())),
                        "requireCanonical": true,
                    }
                ]),
            )
            .ok()
        {
            Some(result) => result,
            None => return Ok(None),
        };
        let operative = decode_evm_address_word(&operative_result, "operative").map_err(|err| {
            Response::error("upstream_invalid_protected_content_verified_listing", &err)
        })?;
        let listing_data = encode_authority_gateway_listing_call(&operative, seller)
            .map_err(|err| Response::error("invalid_protected_content_purchase_request", &err))?;
        let listing_result = match self
            .evm_rpc(
                &source_network,
                "eth_call",
                json!([
                    { "to": market.authority_gateway_contract, "data": listing_data },
                    {
                        "blockHash": format!("0x{}", encode_hex(finalized.finalized_block_hash.as_bytes())),
                        "requireCanonical": true,
                    }
                ]),
            )
            .ok()
        {
            Some(result) => result,
            None => return Ok(None),
        };
        let listing = decode_protected_content_listing(&listing_result).map_err(|err| {
            Response::error("upstream_invalid_protected_content_verified_listing", &err)
        })?;
        let payment_processor = if listing.pay_token != "0x0000000000000000000000000000000000000000"
        {
            let payment_processor_data =
                encode_operatives_payment_processor_call().map_err(|err| {
                    Response::error("invalid_protected_content_purchase_request", &err)
                })?;
            let payment_processor_result = match self
                .evm_rpc(
                    &source_network,
                    "eth_call",
                    json!([
                        { "to": operative, "data": payment_processor_data },
                        {
                            "blockHash": format!("0x{}", encode_hex(finalized.finalized_block_hash.as_bytes())),
                            "requireCanonical": true,
                        }
                    ]),
                )
                .ok()
            {
                Some(result) => result,
                None => return Ok(None),
            };
            Some(
                decode_evm_address_word(&payment_processor_result, "paymentProcessor").map_err(
                    |err| {
                        Response::error("upstream_invalid_protected_content_verified_listing", &err)
                    },
                )?,
            )
        } else {
            None
        };
        Ok(Some(ProtectedContentVerifiedListingObservation {
            chain_id,
            finalized_block_number: finalized.finalized_block_number,
            finalized_block_hash: finalized.finalized_block_hash,
            operative,
            quantity: listing.quantity,
            price: listing.price,
            pay_token: listing.pay_token,
            payment_processor,
        }))
    }

    fn observe_protected_content_rights(
        &self,
        network: &ChainNetwork,
        evidence_rpc_urls: &[String],
        expected_chain_id: u64,
        call: ProtectedContentRightsCall<'_>,
    ) -> Result<ProtectedContentRightsObservation, Response> {
        let started = Instant::now();
        let mut observed: Vec<(&String, ProtectedContentRightsObservation)> = Vec::new();
        for rpc_url in evidence_rpc_urls {
            if let Some(observation) =
                self.observe_protected_content_rights_source(network, rpc_url, call)
            {
                observed.push((rpc_url, observation));
            }
        }
        let mut successful: Vec<ProtectedContentRightsObservation> = observed
            .iter()
            .map(|(_, observation)| *observation)
            .collect();
        if successful
            .iter()
            .any(|observation| observation.chain_id != expected_chain_id)
        {
            return Err(Response::error(
                "conflicting_rights_observations",
                "protected-content evidence sources disagree with configured chain policy",
            ));
        }
        if successful.len() < 2 {
            return Err(Response::error(
                "insufficient_rights_observations",
                "protected-content evidence sources produced fewer than two matching finalized observations",
            ));
        }
        // Independent sources legitimately lag each other by a block or more
        // in what they consider finalized (finality is monotone, so the
        // lowest finalized head is final on every source). When the heads
        // differ, pin every source to that lowest finalized block: the
        // block must be canonical there by hash, and the rights call is
        // re-evaluated at exactly that block, so the corroboration compares
        // the same finalized state everywhere instead of racing finality.
        let heads_agree = successful[1..].iter().all(|observation| {
            observation.finalized_block_number == successful[0].finalized_block_number
                && observation.finalized_block_hash == successful[0].finalized_block_hash
        });
        if !heads_agree {
            let pin = *successful
                .iter()
                .min_by_key(|observation| observation.finalized_block_number)
                .expect("at least two observations");
            let mut pinned = Vec::with_capacity(observed.len());
            for (rpc_url, _) in &observed {
                if let Some(repinned) =
                    self.observe_protected_content_rights_source_at(network, rpc_url, call, &pin)
                {
                    pinned.push(repinned);
                }
            }
            successful = pinned;
            if successful.len() < 2 {
                return Err(Response::error(
                    "insufficient_rights_observations",
                    "protected-content evidence sources produced fewer than two matching finalized observations",
                ));
            }
        }
        let reference = successful[0];
        if successful[1..]
            .iter()
            .any(|observation| *observation != reference)
        {
            return Err(Response::error(
                "conflicting_rights_observations",
                "protected-content evidence sources disagree on finalized rights observation",
            ));
        }
        // Says which block answered and what it answered. A rights call that
        // denies is otherwise indistinguishable from one that was asked about
        // a block too old to know the content, which is exactly the confusion
        // this line exists to remove.
        eprintln!(
            "chain-provider: rights observed block_tag={} block={} block_age_secs={} sources={} outcome={} elapsed_ms={}",
            call.block.tag(),
            reference.finalized_block_number,
            (self.now_unix_seconds)().saturating_sub(reference.finalized_block_timestamp),
            successful.len(),
            match reference.outcome {
                ProtectedContentRightsObservationKind::HasAccess(true) => "has_access",
                ProtectedContentRightsObservationKind::HasAccess(false) => "no_access",
                ProtectedContentRightsObservationKind::Unbound(_) => "unbound",
            },
            started.elapsed().as_millis()
        );
        Ok(reference)
    }

    /// Re-observes one source at a pinned finalized block: the block must be
    /// canonical on that source under the pinned hash, and the rights call
    /// is evaluated at exactly that block.
    fn observe_protected_content_rights_source_at(
        &self,
        network: &ChainNetwork,
        rpc_url: &str,
        call: ProtectedContentRightsCall<'_>,
        pin: &ProtectedContentRightsObservation,
    ) -> Option<ProtectedContentRightsObservation> {
        let mut source_network = network.clone();
        source_network.rpc_url = rpc_url.to_string();
        let block = self
            .evm_rpc(
                &source_network,
                "eth_getBlockByNumber",
                json!([format!("0x{:x}", pin.finalized_block_number), false]),
            )
            .ok()
            .and_then(|value| evm_finalized_block(&value).ok())?;
        if block.finalized_block_number != pin.finalized_block_number
            || block.finalized_block_hash != pin.finalized_block_hash
        {
            return None;
        }
        let outcome = self.protected_content_eth_call_outcome(
            &source_network,
            call,
            &pin.finalized_block_hash,
        )?;
        Some(ProtectedContentRightsObservation {
            chain_id: pin.chain_id,
            finalized_block_number: pin.finalized_block_number,
            finalized_block_hash: pin.finalized_block_hash,
            finalized_block_timestamp: block.finalized_block_timestamp,
            outcome,
        })
    }

    fn observe_protected_content_rights_source(
        &self,
        network: &ChainNetwork,
        rpc_url: &str,
        call: ProtectedContentRightsCall<'_>,
    ) -> Option<ProtectedContentRightsObservation> {
        let mut source_network = network.clone();
        source_network.rpc_url = rpc_url.to_string();
        let chain_id = self
            .evm_rpc(&source_network, "eth_chainId", json!([]))
            .ok()
            .and_then(|value| value.as_str().and_then(|value| parse_hex_u64(value).ok()))?;
        let mut finalized = self
            .evm_rpc(
                &source_network,
                "eth_getBlockByNumber",
                json!([call.block.tag(), false]),
            )
            .ok()
            .and_then(|value| evm_finalized_block(&value).ok())?;
        let outcome = self.protected_content_eth_call_outcome(
            &source_network,
            call,
            &finalized.finalized_block_hash,
        )?;
        finalized.chain_id = chain_id;
        finalized.outcome = outcome;
        Some(finalized)
    }

    fn protected_content_eth_call_outcome(
        &self,
        network: &ChainNetwork,
        call: ProtectedContentRightsCall<'_>,
        finalized_block_hash: &Digest32,
    ) -> Option<ProtectedContentRightsObservationKind> {
        let response = self
            .client
            .post(&network.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "eth_call",
                "params": [
                    { "to": call.contract, "data": call.data },
                    {
                        "blockHash": format!("0x{}", encode_hex(finalized_block_hash.as_bytes())),
                        "requireCanonical": true
                    }
                ],
            }))
            .send()
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let body = response.json::<Value>().ok()?;
        if let Some(error) = body.get("error") {
            return decode_protected_content_unbound_content_id(
                error,
                call.expected_content_access_id,
            )
            .map(ProtectedContentRightsObservationKind::Unbound);
        }
        let result = body.get("result")?.clone();
        decode_evm_bool(&result)
            .ok()
            .map(ProtectedContentRightsObservationKind::HasAccess)
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProtectedContentMintReceiptObservation {
    chain_id: u64,
    receipt_block_number: u64,
    receipt_block_hash: Digest32,
    token_id: String,
    operative: String,
    /// The listing the mint created, read from the `ItemListed` this very
    /// transaction emitted. Carried here so it is covered by the same
    /// two-source agreement as the rest of the receipt, and so the creator tail
    /// needs no second read to learn what it already minted.
    ///
    /// Absent for a free mint, which creates no operative and therefore lists
    /// nothing -- an absence that is the correct outcome, not a missing read.
    listing: Option<ProtectedContentListingRead>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProtectedContentVerifiedListingObservation {
    chain_id: u64,
    finalized_block_number: u64,
    finalized_block_hash: Digest32,
    operative: String,
    quantity: String,
    price: String,
    pay_token: String,
    payment_processor: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProtectedContentRightsObservation {
    chain_id: u64,
    finalized_block_number: u64,
    finalized_block_hash: Digest32,
    finalized_block_timestamp: u64,
    outcome: ProtectedContentRightsObservationKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProtectedContentRightsObservationKind {
    HasAccess(bool),
    Unbound(ContentAccessIdV1),
}

/// Which block a rights call is evaluated at. The two readers of `hasAccess`
/// want genuinely different answers, so neither may assume the other's.
///
/// `Finalized` is for evidence that authorizes releasing a content key: that
/// decision is irreversible once the key is out, so it may only rest on state
/// no reorg can take back.
///
/// `Head` is for the upfront check that decides whether a principal may be
/// shown an asset as theirs. The grant lands in the very block that carries
/// the acquisition -- a mint is readable by `hasAccess` at its own block --
/// while finality on an L2 trails the head by many minutes. Reading that
/// check at `Finalized` would answer "no such content" about a transaction
/// the caller has already watched confirm, which is why this exists.
/// Corroboration is unchanged either way: every source is pinned to one
/// common block and must agree there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProtectedContentRightsBlock {
    Finalized,
    Head,
}

/// One `hasAccess` question: the contract that answers it, the encoded call,
/// the content it is about, and the block it must be evaluated at. These four
/// always travel together -- every source observing the same question must use
/// every one of them identically, or the corroboration compares nothing.
#[derive(Clone, Copy)]
struct ProtectedContentRightsCall<'a> {
    contract: &'a str,
    data: &'a str,
    expected_content_access_id: &'a ContentAccessIdV1,
    block: ProtectedContentRightsBlock,
}

impl ProtectedContentRightsBlock {
    fn tag(self) -> &'static str {
        match self {
            Self::Finalized => "finalized",
            Self::Head => "latest",
        }
    }
}

fn evm_finalized_block(value: &Value) -> Result<ProtectedContentRightsObservation, String> {
    let finalized_block_number = value
        .get("number")
        .and_then(Value::as_str)
        .ok_or_else(|| "EVM block number missing".to_string())
        .and_then(|value| {
            parse_hex_u64(value).map_err(|_| "EVM block number must be a hex quantity".to_string())
        })?;
    let hash = value
        .get("hash")
        .and_then(Value::as_str)
        .ok_or_else(|| "EVM block hash missing".to_string())?;
    let finalized_block_timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .ok_or_else(|| "EVM block timestamp missing".to_string())
        .and_then(|value| {
            parse_hex_u64(value)
                .map_err(|_| "EVM block timestamp must be a hex quantity".to_string())
        })?;
    let bytes = decode_hex(hash, Some(32), "EVM block hash")?;
    Ok(ProtectedContentRightsObservation {
        chain_id: 0,
        finalized_block_number,
        finalized_block_hash: Digest32::new(
            bytes
                .try_into()
                .map_err(|_| "EVM block hash must be 32 bytes".to_string())?,
        ),
        finalized_block_timestamp,
        outcome: ProtectedContentRightsObservationKind::HasAccess(false),
    })
}

/// One question's answer across every source: `Some` only when at least two
/// sources answered it and every source that answered agreed.
///
/// `None` rather than `false` when they did not. For an access check `false`
/// is what draws a Subscribe button, so it must rest on an answer rather than
/// on a silence or a disagreement.
fn corroborated_bool(per_source: &[Vec<Option<bool>>], index: usize) -> Option<bool> {
    let mut answers = per_source
        .iter()
        .filter_map(|source| source.get(index).copied().flatten());
    let first = answers.next()?;
    let mut count = 1;
    for answer in answers {
        if answer != first {
            return None;
        }
        count += 1;
    }
    (count >= 2).then_some(first)
}

fn decode_protected_content_unbound_content_id(
    error: &Value,
    expected_content_access_id: &ContentAccessIdV1,
) -> Option<ContentAccessIdV1> {
    let revert_data = protected_content_revert_data(error)?;
    let bytes = decode_hex(revert_data, Some(36), "protected-content revert data").ok()?;
    if bytes[..4] != PROTECTED_CONTENT_UNBOUND_CONTENT_ID_SELECTOR {
        return None;
    }
    if bytes[4..20] != expected_content_access_id.as_bytes()[..] {
        return None;
    }
    if bytes[20..36].iter().any(|byte| *byte != 0) {
        return None;
    }
    ContentAccessIdV1::new(bytes[4..20].try_into().ok()?).ok()
}

/// The `ROYALTY_SHARE` entries a mint carries, one per payee.
///
/// 1000 units exist per asset: the creator splits 950 and the protocol owner's
/// 50 are minted by the contracts from `CentralStorage.protocolShares()`. Units
/// arrive already in the chain's own denomination, so this validates and
/// formats them and converts nothing.
///
/// An empty list is the default rather than an error: the whole creator share
/// to the creator, which is what every mint did before payees existed.
///
/// Amounts are `0x` quantities — `abi_encode_uint_array_decimal` reads them
/// with `abi_word_hex_quantity` despite its name.
fn protected_content_royalty_units(
    royalties: &[ProtectedContentRoyaltyShare],
    creator: &str,
) -> Result<Vec<(String, String)>, String> {
    if royalties.is_empty() {
        return Ok(vec![(
            creator.to_string(),
            PROTECTED_CONTENT_CREATOR_ROYALTY_SHARE_UNITS.to_string(),
        )]);
    }
    let mut total_units: u32 = 0;
    let mut shares = Vec::with_capacity(royalties.len());
    for royalty in royalties {
        validate_evm_address(&royalty.address)?;
        if royalty.units == 0 {
            return Err("royalty payee share must be greater than zero".to_string());
        }
        total_units = total_units
            .checked_add(royalty.units)
            .ok_or_else(|| "royalty shares overflow".to_string())?;
        shares.push((
            normalize_evm_address(&royalty.address),
            format!("0x{:x}", royalty.units),
        ));
    }
    if total_units != PROTECTED_CONTENT_CREATOR_ROYALTY_UNITS {
        return Err(format!(
            "royalty shares must total {PROTECTED_CONTENT_CREATOR_ROYALTY_UNITS} units"
        ));
    }
    Ok(shares)
}

fn protected_content_revert_data(error: &Value) -> Option<&str> {
    error
        .get("data")
        .and_then(Value::as_str)
        .or_else(|| {
            error
                .get("data")
                .and_then(|value| value.get("data"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            error
                .get("data")
                .and_then(|value| value.get("originalError"))
                .and_then(|value| value.get("data"))
                .and_then(Value::as_str)
        })
}

fn validate_finalized_observation_freshness(
    finalized_block_timestamp: u64,
    now_unix_seconds: u64,
    max_age_secs: u64,
    max_future_skew_secs: u64,
) -> Result<(), String> {
    if finalized_block_timestamp > now_unix_seconds {
        let skew = finalized_block_timestamp - now_unix_seconds;
        if skew > max_future_skew_secs {
            return Err("finalized observation is too far in the future".to_string());
        }
        return Ok(());
    }
    let age = now_unix_seconds - finalized_block_timestamp;
    if age > max_age_secs {
        return Err("finalized observation is too stale".to_string());
    }
    Ok(())
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

fn parse_encrypted_content_identity(value: &str) -> Result<EncryptedContentIdentityV1, Response> {
    let bytes = decode_hex(value, None, "encrypted_content").map_err(|_| {
        Response::error(
            "invalid_rights_policy_request",
            "protected-content rights policy request is invalid",
        )
    })?;
    EncryptedContentIdentityV1::from_canonical_bytes(&bytes).map_err(|_| {
        Response::error(
            "invalid_rights_policy_request",
            "protected-content rights policy request is invalid",
        )
    })
}

fn parse_content_access_id(value: &str) -> Result<ContentAccessIdV1, Response> {
    let bytes = decode_hex(value, Some(16), "content_access_id").map_err(|_| {
        Response::error(
            "invalid_rights_policy_request",
            "protected-content rights policy request is invalid",
        )
    })?;
    let bytes: [u8; 16] = bytes.try_into().map_err(|_| {
        Response::error(
            "invalid_rights_policy_request",
            "protected-content rights policy request is invalid",
        )
    })?;
    ContentAccessIdV1::new(bytes).map_err(|_| {
        Response::error(
            "invalid_rights_policy_request",
            "protected-content rights policy request is invalid",
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

        let request = match decode_request_frame(&line) {
            Ok(request) => request,
            Err(response) => {
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
