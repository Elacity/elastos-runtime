use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

use elastos_protected_content_contracts::{EvmRightsMethodAbiV1, RightsActionV1};

pub(super) const NODE_LIFECYCLE_STATE_SCHEMA: &str = "elastos.chain.node_lifecycle_state/v1";
pub(super) const PROTECTED_CONTENT_POLICY_SCHEMA: &str =
    "elastos.chain.protected-content-policy/v1";
pub(super) const PROTECTED_CONTENT_CREATOR_MINT_SCHEMA: &str =
    "elastos.chain.protected-content-creator-mint/v1";
pub(super) const PROTECTED_CONTENT_MINT_RECEIPT_SCHEMA: &str =
    "elastos.chain.protected-content-mint-receipt/v1";
pub(super) const PROTECTED_CONTENT_VERIFIED_LISTING_SCHEMA: &str =
    "elastos.chain.protected-content-verified-listing/v1";
pub(super) const PROTECTED_CONTENT_PURCHASE_SCHEMA: &str =
    "elastos.chain.protected-content-purchase/v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ChainKind {
    EvmJsonRpc,
    MainchainRest,
    BitcoinCoreRpc,
    BitcoinRest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChainNetwork {
    pub(super) id: String,
    pub(super) display_name: String,
    pub(super) kind: ChainKind,
    #[serde(default)]
    pub(super) chain_id: Option<u64>,
    pub(super) native_symbol: String,
    pub(super) provider: String,
    pub(super) mainnet: bool,
    #[serde(default)]
    pub(super) explorer_url: Option<String>,
    pub(super) rpc_url: String,
    #[serde(default)]
    pub(super) rights_methods: Vec<RightsMethod>,
    #[serde(default)]
    pub(super) protected_content_creator_mint: Option<ProtectedContentCreatorMintMethod>,
    #[serde(default)]
    pub(super) protected_content_market: Option<ProtectedContentMarketMethod>,
}

impl ChainNetwork {
    pub(super) fn public_view(&self) -> Value {
        json!({
            "id": self.id,
            "display_name": self.display_name,
            "kind": self.kind,
            "chain_id": self.chain_id,
            "native_symbol": self.native_symbol,
            "provider": self.provider,
            "mainnet": self.mainnet,
            "explorer_url": self.explorer_url,
            "configured": !self.rpc_url.trim().is_empty(),
            "rights_methods": self.rights_methods.iter().map(RightsMethod::public_view).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RightsMethod {
    pub(super) id: String,
    pub(super) contract: String,
    pub(super) abi: RightsMethodAbi,
    pub(super) selector: String,
    #[serde(default)]
    pub(super) protected_content_policies: Vec<ProtectedContentPolicySource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ProtectedContentCreatorMintMethod {
    pub(super) ledger: String,
    pub(super) pay_token: String,
    pub(super) asset_created_emitter: String,
    pub(super) abi: ProtectedContentCreatorMintAbi,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProtectedContentCreatorMintAbi {
    ElacityMintV1,
}

impl ProtectedContentCreatorMintAbi {
    pub(super) const fn selector(self) -> &'static str {
        "0x47cbeeb4"
    }

    pub(super) const fn function(self) -> &'static str {
        "mint(string,uint16,bytes,bytes)"
    }

    pub(super) const fn asset_created_topic0(self) -> &'static str {
        "0xc0a995e4052be044599af577ab2f3382d67bd34df95a76226e7c464e9d4dba46"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ProtectedContentMarketMethod {
    pub(super) authority_gateway_contract: String,
    pub(super) evidence_rpc_urls: Vec<String>,
}

impl RightsMethod {
    fn public_view(&self) -> Value {
        json!({
            "id": self.id,
            "abi": self.abi,
            "configured": true,
        })
    }
}

impl RightsMethodAbi {
    pub(super) const fn to_contract_abi(self) -> EvmRightsMethodAbiV1 {
        match self {
            Self::HasAccessByContentIdAddressBytes16 => {
                EvmRightsMethodAbiV1::HasAccessByContentIdAddressBytes16
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProtectedContentPolicyAction {
    View,
    Stream,
    Download,
    Execute,
}

impl ProtectedContentPolicyAction {
    pub(super) const fn to_contract_action(self) -> RightsActionV1 {
        match self {
            Self::View => RightsActionV1::View,
            Self::Stream => RightsActionV1::Stream,
            Self::Download => RightsActionV1::Download,
            Self::Execute => RightsActionV1::Execute,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ProtectedContentPolicySource {
    pub(super) action: ProtectedContentPolicyAction,
    pub(super) evidence_rpc_urls: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum RightsMethodAbi {
    HasAccessByContentIdAddressBytes16,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Networks,
    Status {
        network: String,
    },
    BlockNumber {
        network: String,
    },
    SyncHealth {
        network: String,
    },
    Balance {
        network: String,
        address: String,
        #[serde(default)]
        block: Option<String>,
    },
    ContractCall {
        network: String,
        to: String,
        data: String,
        #[serde(default)]
        block: Option<String>,
    },
    EstimateGas {
        network: String,
        from: String,
        to: String,
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        data: Option<String>,
    },
    TransactionCount {
        network: String,
        address: String,
        #[serde(default)]
        block: Option<String>,
    },
    GasPrice {
        network: String,
    },
    FeeHistory {
        network: String,
        block_count: String,
        newest_block: String,
        #[serde(default)]
        reward_percentiles: Vec<f64>,
    },
    Code {
        network: String,
        address: String,
        #[serde(default)]
        block: Option<String>,
    },
    Logs {
        network: String,
        filter: Value,
    },
    Transaction {
        network: String,
        hash: String,
    },
    Receipt {
        network: String,
        hash: String,
    },
    ProtectedContentRightsEvidence {
        signed_runtime_release_operation: String,
    },
    ResolveProtectedContentPolicy {
        encrypted_content: String,
        content_access_id: String,
        action: ProtectedContentPolicyAction,
    },
    ResolveProtectedContentCreatorMint {
        creator: String,
        token_uri: String,
        content_access_id: String,
        copies: String,
        price: String,
    },
    ResolveProtectedContentMintReceipt {
        network: String,
        hash: String,
        creator: String,
        ledger: String,
        token_uri: String,
        op_type_code: u16,
    },
    ResolveProtectedContentVerifiedListing {
        network: String,
        seller: String,
        ledger: String,
        token_id: String,
    },
    ResolveProtectedContentPurchase {
        seller: String,
        chain_namespace: String,
        network: String,
        ledger: String,
        token_id: String,
    },
    Proof {
        network: String,
        proof_kind: ChainProofKind,
        subject: String,
    },
    Erc1271IsValidSignature {
        network: String,
        contract: String,
        message_hash: String,
        signature: String,
    },
    PrepareTransaction {
        network: String,
        from: String,
        to: String,
        value: String,
        #[serde(default)]
        data: Option<String>,
    },
    BroadcastTransaction {
        network: String,
        signed_transaction: String,
    },
    NodeLifecycle {
        network: String,
        action: NodeLifecycleAction,
    },
    Shutdown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ChainProofKind {
    Status,
    SyncHealth,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum NodeLifecycleAction {
    Status,
    Start,
    Stop,
    Restart,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum NodeLifecycleStateKind {
    NotConfigured,
    ExternalLoopback,
    ManagedLocal,
    RemoteBackend,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct NodeSupervisorConfig {
    #[serde(default)]
    pub(super) networks: BTreeMap<String, NodeSupervisorNetworkConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct NodeSupervisorNetworkConfig {
    pub(super) start: NodeSupervisorCommand,
    pub(super) stop: NodeSupervisorCommand,
    pub(super) restart: NodeSupervisorCommand,
    #[serde(default)]
    pub(super) timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct NodeSupervisorCommand {
    pub(super) program: String,
    #[serde(default)]
    pub(super) args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PersistedNodeLifecycleState {
    pub(super) state: NodeLifecycleStateKind,
    pub(super) managed: bool,
    pub(super) first_seen_at: u64,
    pub(super) updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct NodeLifecycleStateFile {
    pub(super) schema: String,
    #[serde(default)]
    pub(super) networks: BTreeMap<String, PersistedNodeLifecycleState>,
}

impl Default for NodeLifecycleStateFile {
    fn default() -> Self {
        Self {
            schema: NODE_LIFECYCLE_STATE_SCHEMA.to_string(),
            networks: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl Response {
    pub(super) fn ok(data: Value) -> Self {
        Response::Ok { data: Some(data) }
    }

    pub(super) fn empty_ok() -> Self {
        Response::Ok { data: None }
    }

    pub(super) fn error(code: &str, message: &str) -> Self {
        Response::Error {
            code: code.to_string(),
            message: message.to_string(),
        }
    }
}
