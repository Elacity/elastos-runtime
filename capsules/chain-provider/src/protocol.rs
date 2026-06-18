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
    /// Additional RPC endpoints tried (in order) when the primary `rpc_url` fails over
    /// (transport error / HTTP 5xx-4xx / JSON-RPC error). Mirrors PC2's round-robin Base
    /// RPC pool (`src/utils/rpc.ts`): the primary MUST be a key-less, rate-tolerant
    /// endpoint; keyed providers belong at the back. A single point of RPC failure
    /// silently degrades the rights read to "not owned", so the pool is fail-soft on
    /// transport while the answer itself stays fail-closed.
    #[serde(default)]
    pub(super) rpc_fallback_urls: Vec<String>,
    /// RPC endpoints permitted for `eth_getLogs` (channel discovery / event scans). A SUBSET
    /// of the pool: many free Base endpoints cap (or refuse) `eth_getLogs` to tiny block
    /// ranges (e.g. `1rpc.io` → 50 blocks, `blastapi` → 10, `meowrpc` → unsupported), which
    /// would make the chunked factory scan fail. When non-empty, log queries route ONLY here
    /// (operator head + range-capable publics); when empty, they fall back to the full pool.
    #[serde(default)]
    pub(super) log_query_rpc_urls: Vec<String>,
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
    /// Discover the dDRM channels (DigitalAsset collections) a creator owns by scanning the
    /// channel factory's `ChannelCreated` logs filtered to the creator's address. READ-ONLY
    /// (`eth_getLogs`); no keys. The factory + event topic + scan-from block default to the
    /// real Base values (overridable) so the app never names a contract address itself.
    ListChannels {
        network: String,
        #[serde(default)]
        factory: Option<String>,
        creator: String,
        #[serde(default)]
        from_block: Option<String>,
    },
    /// Assemble the `createChannel(uint8,uint8,string,string,bytes)` calldata (PURE: no RPC,
    /// no keys) — the `{ to, data, value }` an external signer (wallet-provider) signs and
    /// `broadcast_transaction` sends to deploy a new channel. Mirrors `AssembleMint`.
    AssembleCreateChannel {
        channel: Box<CreateChannelAssembly>,
    },
    /// Assemble the post-mint trade-enabling approval (PC2's 2nd mint tx). READ-then-PURE:
    /// discover the just-minted asset's operative contract from its `AssetCreated` log
    /// (`_to == creator`, `_channel == channel`), read the channel's `authority()` gateway,
    /// and — unless the gateway is already approved — ABI-encode `setApprovalForAll(gateway,
    /// true)` on the operative. Never signs/broadcasts; fails closed if no confirmed mint is
    /// found yet (the caller retries once it is mined).
    AssembleTradeApproval {
        network: String,
        channel: String,
        creator: String,
        /// PIN the approval to the JUST-MINTED asset by its `bytes16` content id (KID). When
        /// present, the operative is resolved from the `AssetCreated` whose mint transaction
        /// embeds THIS content id in `opRawData` — never the channel's newest mint — so a
        /// freshly-minted asset is never falsely reported tradable because an EARLIER asset in
        /// the same channel was already approved. Absent ⇒ legacy newest-in-channel resolution.
        #[serde(default)]
        content_id: Option<String>,
        /// FAST PATH: the broadcast mint TRANSACTION hash (from the owner's wallet approval). When
        /// present, the operative + token id are resolved from THAT transaction's own receipt
        /// (`eth_getTransactionReceipt` → its `AssetCreated` log) in ONE cheap call, instead of a
        /// wide `eth_getLogs` scan that public RPCs rate-limit/range-cap. The owner's wallet still
        /// signs+broadcasts the mint (no delegation); this only READS the receipt of that tx. If
        /// the receipt is not available yet (pending) or does not match, it falls back to the scan.
        #[serde(default)]
        tx_hash: Option<String>,
    },
    Shutdown,
}

/// The structured `createChannel` the chain capability ABI-encodes. The selector + factory
/// are supplied (configured), exactly like the mint selector — keccak is not computed
/// in-capsule.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateChannelAssembly {
    /// The configured 4-byte `createChannel` selector (default `0xc384baa2`).
    pub selector: String,
    /// The channel factory contract (createChannel `to`).
    pub factory: String,
    /// 0=…, channel type code (PC2 `_channelType`).
    pub channel_type: u8,
    /// Channel scope code (PC2 `_scope`).
    pub scope: u8,
    /// Human channel name (`_name`).
    pub name: String,
    /// Channel metadata token URI (`_tokenURI`).
    pub token_uri: String,
    /// Extra `bytes data` arg (default empty bytes).
    #[serde(default)]
    pub data_hex: Option<String>,
    /// The payable `channelCreationFee` (hex quantity) the runtime read from CENTRAL_STORAGE;
    /// the pure assembler never reads chain state. Defaults to `0x0`.
    #[serde(default)]
    pub value_wei: Option<String>,
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
