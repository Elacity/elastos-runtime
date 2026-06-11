use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) const NODE_LIFECYCLE_STATE_SCHEMA: &str = "elastos.chain.node_lifecycle_state/v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ChainKind {
    EvmJsonRpc,
    MainchainRest,
    BitcoinCoreRpc,
    BitcoinRest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum RightsMethodAbi {
    /// Real Base ABI: `hasAccessByContentId(address holder, bytes16 contentId)`
    /// (selector `0x54d42821`). The production rights read.
    HasAccessByContentIdAddressBytes16,
    /// Legacy/guessed `(string,address,string)` shape — kept for config flexibility and
    /// the local CID-keyed mock loop, but NOT the real Base ABI.
    HasAccessByContentIdStringAddressString,
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
    HasAccessByContentId {
        network: String,
        contract: String,
        content_id: String,
        subject: String,
        right: String,
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
    /// Assemble the dDRM content-mint calldata (PURE: no RPC, no keys). Turns a
    /// publish-provider `UnsignedMintV1` into the `{ to, data, value }` an external
    /// signer (wallet-provider) signs and `broadcast_transaction` sends.
    AssembleMint {
        mint: Box<MintAssembly>,
    },
    Shutdown,
}

/// The structured mint the chain capability ABI-encodes (publish-provider's
/// `UnsignedMintV1`, plus the configured selector + the fee value the runtime read).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MintAssembly {
    /// The configured 4-byte mint selector (`keccak256("mint(string,uint16,bytes,bytes)")
    /// [..4]`) — supplied, not computed, mirroring the `has_access` selector.
    pub selector: String,
    /// The creator's Channel contract (mint `to`).
    pub to: String,
    /// `_uri` = `{metadataCid}/metadata.json`.
    pub token_uri: String,
    /// 0=FREE, 1=BUY_ONCE, 2=BUY_AND_RESELL.
    pub op_type_code: u16,
    /// On-chain `bytes16 contentId` (`0x` + 32 hex == KID).
    pub content_id: String,
    /// The payable mint fee (hex quantity) the runtime read from CENTRAL_STORAGE; the
    /// pure assembler never reads chain state. Defaults to `0x0`.
    #[serde(default)]
    pub value_wei: Option<String>,
    /// Paid listings only: the `opRawData` payee/royalty arrays + metadata URI.
    #[serde(default)]
    pub op_raw: Option<MintOpRaw>,
    /// Paid listings only: the `sellRawData` sale terms.
    #[serde(default)]
    pub sell: Option<MintSell>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MintOpRaw {
    /// `ipfs://{metadataCid}` (the folder root, app.js:1593).
    pub metadata_uri: String,
    pub addresses: Vec<String>,
    pub role_types: Vec<u64>,
    pub amounts: Vec<String>,
    /// Present (and encoded as a trailing `uint16`) only for BUY_AND_RESELL.
    #[serde(default)]
    pub reseller_cut: Option<u16>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MintSell {
    pub copies: String,
    pub price_wei: String,
    pub pay_token: String,
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
