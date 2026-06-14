//! Create portal capability routes (the producer half of the dDRM seam).
//!
//! These routes are how the de-privileged `creator` app frame mints a dDRM asset on
//! PQ custody WITHOUT ever holding a key. The frame ships bytes + listing terms under
//! its Home launch capability; the runtime orchestrates the already-proven producer
//! spine and hands back an UNSIGNED mint for the USER's wallet to sign + broadcast.
//! The runtime never signs and never broadcasts here — minting is the owner's act.
//!
//!   GET  /api/apps/creator/status
//!        -> { ready, quorum, node_count, protection_type }   (no secrets, no keys)
//!   POST /api/apps/creator/prepare-mint   (JSON: { file_b64, meta })
//!        -> { content_id, kid, asset_cid, metadata_cid, unsigned_mint, protections,
//!             next:"sign_with_wallet" }                       (no key material)
//!
//! THE SPINE (each step a capability-clean provider call):
//!   encrypt seal_inline_threshold  -> mint CEK + CENC-encrypt + SHAMIR-split + seal a
//!                                     share to EACH quorum node (CEK never leaves the
//!                                     encrypt-provider boundary; only sealed shares +
//!                                     the node-set pin come back).
//!   content publish (file)         -> pin the encrypted segment, get the asset CID.
//!   <assemble metadata envelope>   -> the CEK custody block is the dKMS escrow
//!                                     descriptor (cenc:elastos-pq-hybrid-threshold-v0),
//!                                     swapped in EXACTLY where PC2 wrote Lit.
//!   content publish (directory)    -> pin metadata.json, get the metadata CID.
//!   publish prepare_publish        -> assemble the unsigned mint (contentId==bytes16 KID).
//!
//! Containment invariant: the sealed shares ARE public escrow ciphertext (only the
//! quorum can unwrap them under rights), so they belong in the public protections
//! block — the dKMS analogue of PC2's public `litCiphertext`. But a RAW CEK / share /
//! seed must never appear; we fail closed (defense in depth) if one ever did.

use std::path::Path as FsPath;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use elastos_runtime::provider::ProviderRegistry;
use serde::Deserialize;
use serde_json::{json, Value};

use super::gateway::{require_home_launch_token_for_any_context, GatewayState};

/// The de-privileged app this route serves. A launch token for any other context is refused.
const CREATOR_APP: &str = "creator";

/// The dKMS threshold protection type — the CEK-custody scheme that replaces Lit.
const THRESHOLD_PROTECTION_TYPE: &str = "cenc:elastos-pq-hybrid-threshold-v0";
const THRESHOLD_SCHEME: &str = "elastos-pq-hybrid-threshold-v0";
const ENVELOPE_SCHEMA: &str = "elastos.asset.envelope/v1";

/// Env override for the PUBLIC-ONLY quorum descriptor; defaults to `<data_dir>/dkms/quorum.json`.
const QUORUM_DESCRIPTOR_ENV: &str = "ELASTOS_DKMS_QUORUM_DESCRIPTOR";

/// Real Base selector for `mint(string,uint16,bytes,bytes)` — pinned (not computed) and
/// handed to the pure assembler, identical to `mint_authority`. Overridable for other deployments.
const DEFAULT_MINT_SELECTOR: &str = "0x47cbeeb4";

/// The real Base ChannelFactory (`pc2-node config/default.json` `contracts.v3.channel_factory`).
/// Overridable via `ELASTOS_DDRM_CHANNEL_FACTORY`. The app NEVER names this itself.
const DEFAULT_CHANNEL_FACTORY: &str = "0xE1365ed47353De2F8A6a69E271e36650A9EE368F";
/// The real Base CentralStorage (`contracts.v3.central_storage`), source of the
/// `channelCreationFee()`. Overridable via `ELASTOS_DDRM_CENTRAL_STORAGE`.
const DEFAULT_CENTRAL_STORAGE: &str = "0x0C1EeA2A3361B80AC0e42179335dB536A951760b";
/// `createChannel(uint8,uint8,string,string,bytes)` selector — pinned (not computed),
/// handed to the pure assembler. Overridable via `ELASTOS_DDRM_CREATE_CHANNEL_SELECTOR`.
const DEFAULT_CREATE_CHANNEL_SELECTOR: &str = "0xc384baa2";
/// `channelCreationFee() returns (uint256 fee, address token)` selector — read on-chain
/// (eth_call) to set the payable `value`, exactly as PC2's creator does.
const CHANNEL_CREATION_FEE_SELECTOR: &str = "0x4218a471";
/// The empty channel config `bytes data`: `abi.encode((address,uint256)[] royalties,
/// (uint8,address,uint256,uint256,bool)[] plans, (address,uint256)[] tokenAccess)` with all
/// three arrays empty — a channel with no royalty splits, no subscription plans, no token
/// gates. The minimal valid config; richer config is a follow-on (PC2's plan/royalty UI).
const EMPTY_CHANNEL_CONFIG_DATA: &str = "0x0000000000000000000000000000000000000000000000000000000000000060000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000a0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
/// PC2 channel-type/scope codes (`elacity-creator/app.js`): STANDARD channel, PRIVATE scope
/// (subscription-gated) is the creator default.
const CHANNEL_TYPE_STANDARD: u8 = 1;
const CHANNEL_SCOPE_PRIVATE: u8 = 2;
const CHANNEL_SCOPE_PUBLIC: u8 = 1;

fn channel_factory() -> String {
    std::env::var("ELASTOS_DDRM_CHANNEL_FACTORY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CHANNEL_FACTORY.to_string())
}

fn central_storage() -> String {
    std::env::var("ELASTOS_DDRM_CENTRAL_STORAGE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CENTRAL_STORAGE.to_string())
}

fn create_channel_selector() -> String {
    std::env::var("ELASTOS_DDRM_CREATE_CHANNEL_SELECTOR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CREATE_CHANNEL_SELECTOR.to_string())
}

/// The mint chain. Defaults to Base mainnet; overridable via `ELASTOS_DDRM_CHAIN_ID`.
/// The producer signs in the OWNER's wallet, so the linked external account must live here.
fn mint_chain_id() -> u64 {
    std::env::var("ELASTOS_DDRM_CHAIN_ID")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(8453)
}

fn mint_chain_namespace() -> String {
    format!("eip155:{}", mint_chain_id())
}

/// Whether a linked account's chain namespace can sign for the mint chain. Mirrors the
/// wallet-provider's own `chain_namespaces_compatible` (`capsules/wallet-provider/src/
/// validation.rs`): an EVM address is the SAME key on every `eip155:*` chain, so any
/// linked EVM account (e.g. linked while MetaMask was on mainnet `eip155:1`) is valid for
/// the Base mint — the metamask connector switches the provider to the tx's chain at sign
/// time. Non-EVM namespaces must match exactly. Keeping this in lockstep with the wallet
/// provider is the One Canonical Path (#10): the Create portal must not be stricter than
/// the layer that actually signs.
fn chain_namespace_can_sign_mint(account_namespace: &str) -> bool {
    let mint = mint_chain_namespace();
    account_namespace == mint
        || (account_namespace.starts_with("eip155:") && mint.starts_with("eip155:"))
}

/// chain-provider network slug for the mint chain (used only to name the broadcast resource
/// in the approval audit trail — the OWNER's wallet does the actual broadcast).
fn mint_network() -> &'static str {
    match mint_chain_id() {
        20 => "esc-mainnet",
        _ => "base-mainnet",
    }
}

fn mint_selector() -> String {
    std::env::var("ELASTOS_DDRM_MINT_SELECTOR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MINT_SELECTOR.to_string())
}

/// Field names that must NEVER appear on a creator response or in a published envelope —
/// their presence means a raw secret escaped a boundary, so we refuse rather than surface it.
/// (Sealed shares — `wrapped_share_b64` — and verifying keys are PUBLIC escrow material and
/// are intentionally NOT on this list.)
const FORBIDDEN_RAW_KEY_FIELDS: &[&str] = &[
    "raw_cek",
    "cek",
    "plaintext_b64",
    "private_key",
    "seed",
    "master_seed",
    "master_seed_b64",
    "signer_seed",
];

/// One quorum node read from the PUBLIC-ONLY descriptor: the identity the producer seals
/// shares to. No secrets — recipient key (where the share is sealed) + verifying key (pins
/// the node-set id the open must match).
#[derive(Debug, Clone)]
struct QuorumNode {
    verifying_key_b64: String,
    recipient_pub_b64: String,
}

/// Listing terms the frame submits alongside the file. The frame holds no authority; these
/// are plain hints the runtime validates and binds into the (public) envelope + mint.
/// camelCase on the wire (the frame is JS) — `isMedia`, `creatorAddress`, `fileName`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MintMeta {
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    price: String,
    #[serde(default)]
    currency: String,
    #[serde(default)]
    channel: String,
    #[serde(default)]
    mime: String,
    #[serde(default)]
    is_media: bool,
    /// The creator's EVM payout address — REQUIRED for a paid mint (the wallet that will sign).
    #[serde(default)]
    creator_address: String,
    #[serde(default)]
    file_name: String,
}

/// The prepare-mint request body: the asset bytes (base64) + listing terms. Base64-in-JSON
/// avoids a multipart dependency; non-media objects are small. (Media, when wired, will
/// stream through a content/encode route rather than the JSON body.)
#[derive(Debug, Deserialize)]
pub struct PrepareMintRequest {
    file_b64: String,
    #[serde(default)]
    meta: MintMeta,
}

// ── GET /api/apps/creator/status ───────────────────────────────────────────────
/// Report whether the Create capability is ready: the launch token is valid for the
/// creator app AND a PUBLIC-ONLY quorum descriptor is present. Carries no key material.
pub async fn creator_status(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    if require_home_launch_token_for_any_context(&state.data_dir, &headers, &[CREATOR_APP]).is_err()
    {
        return error_json(StatusCode::UNAUTHORIZED, "missing or invalid home launch token");
    }
    match load_quorum_descriptor(&state.data_dir) {
        Ok(nodes) => Json(json!({
            "schema": "elastos.creator.status/v1",
            "ready": true,
            "quorum": format!("2-of-{} dKMS", nodes.len()),
            "node_count": nodes.len(),
            "protection_type": THRESHOLD_PROTECTION_TYPE,
        }))
        .into_response(),
        Err(message) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "ready": false, "error": message })),
        )
            .into_response(),
    }
}

// ── POST /api/apps/creator/prepare-mint ────────────────────────────────────────
/// Drive the producer spine and return the UNSIGNED mint for the user's wallet. The
/// runtime never signs; it prepares. Media (CENC/DASH) is a deferred branch — this
/// path handles single-object (non-media) assets and fails closed for media.
pub async fn creator_prepare_mint(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Json<PrepareMintRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let ctx =
        match require_home_launch_token_for_any_context(&state.data_dir, &headers, &[CREATOR_APP]) {
            Ok(ctx) => ctx,
            Err(_) => {
                return error_json(StatusCode::UNAUTHORIZED, "missing or invalid home launch token")
            }
        };
    let Some(registry) = state.provider_registry.as_ref() else {
        return staged_error(StatusCode::SERVICE_UNAVAILABLE, "encrypt", "providers unavailable");
    };

    let Json(req) = match body {
        Ok(json) => json,
        Err(rejection) => {
            return staged_error(StatusCode::BAD_REQUEST, "encrypt", &rejection.body_text())
        }
    };
    let meta = req.meta;
    let file_bytes = match base64::engine::general_purpose::STANDARD.decode(req.file_b64.trim()) {
        Ok(bytes) => bytes,
        Err(_) => return staged_error(StatusCode::BAD_REQUEST, "encrypt", "file_b64 is not valid base64"),
    };
    if file_bytes.is_empty() {
        return staged_error(StatusCode::BAD_REQUEST, "encrypt", "no file was uploaded");
    }
    if meta.title.trim().is_empty() {
        return staged_error(StatusCode::BAD_REQUEST, "publish", "a title is required");
    }
    let nodes = match load_quorum_descriptor(&state.data_dir) {
        Ok(nodes) => nodes,
        Err(message) => return staged_error(StatusCode::SERVICE_UNAVAILABLE, "encrypt", &message),
    };

    // Media (video/audio) takes the DASH/CENC branch: package -> per-track CENC under ONE
    // asset CEK -> publish a DASH directory. Non-media objects take the single-segment branch.
    let outcome = if meta.is_media {
        run_prepare_mint_media(registry, &ctx.principal_id, &file_bytes, &meta, &nodes).await
    } else {
        run_prepare_mint(registry, &ctx.principal_id, &file_bytes, &meta, &nodes).await
    };

    match outcome {
        Ok(result) => {
            // Defense in depth: never emit a response carrying a raw secret.
            if assert_no_raw_key_material(&result).is_err() {
                return staged_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "encrypt",
                    "refused: prepared mint carried raw key material",
                );
            }
            // Persist into the creator's Library (best-effort) so the asset lists at the
            // correct location and opens on double-click — PC2 parity for a creator's own mint.
            persist_minted_asset_to_library(
                &state.data_dir,
                &ctx.principal_id,
                &meta,
                &file_bytes,
                &result,
            );
            Json(result).into_response()
        }
        Err(staged) => staged_error(StatusCode::BAD_GATEWAY, &staged.stage, &staged.message),
    }
}

// ── GET /api/apps/creator/wallet ───────────────────────────────────────────────
/// Surface the principal's linked wallet account(s) on the mint chain so the frame can show
/// the connected address and use it for channel discovery + as the mint/deploy signer. The
/// frame holds NO wallet authority (#3); it only learns which already-linked address to bind.
/// Returns addresses ONLY — no keys, no account secrets.
pub async fn creator_wallet(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    let ctx =
        match require_home_launch_token_for_any_context(&state.data_dir, &headers, &[CREATOR_APP]) {
            Ok(ctx) => ctx,
            Err(_) => {
                return error_json(StatusCode::UNAUTHORIZED, "missing or invalid home launch token")
            }
        };
    let Some(registry) = state.provider_registry.as_ref() else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "providers unavailable");
    };
    let namespace = mint_chain_namespace();
    let accounts = match provider_data(
        registry,
        "wallet",
        &json!({ "op": "accounts", "principal_id": ctx.principal_id }),
    )
    .await
    {
        Ok(value) => value,
        Err(message) => return error_json(StatusCode::BAD_GATEWAY, &message),
    };
    let mut addresses: Vec<String> = accounts
        .get("accounts")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter(|a| {
                    a.get("chain_namespace")
                        .and_then(Value::as_str)
                        .is_some_and(chain_namespace_can_sign_mint)
                })
                .filter_map(|a| a.get("address").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    // The same EVM address may be linked under several eip155 chains — show it once.
    addresses.sort();
    addresses.dedup();
    Json(json!({
        "schema": "elastos.creator.wallet/v1",
        "network": mint_network(),
        "chain_namespace": namespace,
        "addresses": addresses,
    }))
    .into_response()
}

// ── GET /api/apps/creator/channels?creator=0x… ─────────────────────────────────
/// The query for channel discovery: the creator's EVM wallet address (the signer-to-be).
#[derive(Debug, Deserialize)]
pub struct ChannelsQuery {
    #[serde(default)]
    creator: String,
}

/// Discover the channels the creator already owns on the mint chain, so the Create portal
/// can offer them for selection (PC2 `loadChannels`). READ-ONLY: the chain provider scans
/// `ChannelCreated` logs filtered to the creator's address — no keys, no signing, no ambient
/// authority. Fail-closed: a wallet address is REQUIRED (no silent "all channels" scan).
pub async fn creator_list_channels(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Query(query): Query<ChannelsQuery>,
) -> Response {
    if require_home_launch_token_for_any_context(&state.data_dir, &headers, &[CREATOR_APP]).is_err()
    {
        return error_json(StatusCode::UNAUTHORIZED, "missing or invalid home launch token");
    }
    let Some(registry) = state.provider_registry.as_ref() else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "providers unavailable");
    };
    let creator = query.creator.trim();
    if creator.is_empty() {
        return error_json(
            StatusCode::BAD_REQUEST,
            "a creator wallet address (0x…) is required — connect your wallet first",
        );
    }
    let req = json!({
        "op": "list_channels",
        "network": mint_network(),
        "factory": channel_factory(),
        "creator": creator,
    });
    match provider_data(registry, "chain", &req).await {
        Ok(data) => Json(json!({
            "schema": "elastos.creator.channels/v1",
            "network": mint_network(),
            "factory": data.get("factory").cloned().unwrap_or_else(|| json!(channel_factory())),
            "creator": data.get("creator").cloned().unwrap_or_else(|| json!(creator)),
            "channels": data.get("channels").cloned().unwrap_or_else(|| json!([])),
            // Resumable cursor state: the UI re-polls while `indexing` is true so deep
            // (older) channels surface across calls without re-scanning history each time.
            "indexing": data.get("indexing").cloned().unwrap_or(json!(false)),
            "scanned_floor": data.get("scanned_floor").cloned().unwrap_or(Value::Null),
            "deploy_block": data.get("deploy_block").cloned().unwrap_or(Value::Null),
            "latest_block": data.get("latest_block").cloned().unwrap_or(Value::Null),
        }))
        .into_response(),
        Err(message) => error_json(StatusCode::BAD_GATEWAY, &message),
    }
}

// ── POST /api/apps/creator/create-channel ──────────────────────────────────────
/// The create-channel request: a name, optional public/private scope, and the creator's
/// wallet (the signer). Subscription plans / royalties / token gates are a follow-on; this
/// creates a minimal STANDARD channel (empty config).
#[derive(Debug, Deserialize)]
pub struct CreateChannelRequest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    /// `"public"` or `"private"` (default). Maps to PC2's CHANNEL_SCOPE.
    #[serde(default)]
    scope: String,
    /// The creator's EVM wallet address — REQUIRED (the wallet that signs the deploy).
    #[serde(default)]
    creator_address: String,
}

/// Prepare an UNSIGNED `createChannel` for the owner's wallet (PC2 parity). The runtime
/// publishes minimal channel metadata, reads the `channelCreationFee` on-chain, assembles
/// the pure calldata, binds the owner's linked wallet, and enqueues the MetaMask approval.
/// The runtime signs NOTHING and deploys NOTHING — the channel is the owner's act.
pub async fn creator_create_channel(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Json<CreateChannelRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let ctx =
        match require_home_launch_token_for_any_context(&state.data_dir, &headers, &[CREATOR_APP]) {
            Ok(ctx) => ctx,
            Err(_) => {
                return error_json(StatusCode::UNAUTHORIZED, "missing or invalid home launch token")
            }
        };
    let Some(registry) = state.provider_registry.as_ref() else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "providers unavailable");
    };
    let Json(req) = match body {
        Ok(json) => json,
        Err(rejection) => return error_json(StatusCode::BAD_REQUEST, &rejection.body_text()),
    };
    match run_create_channel(registry, &ctx.principal_id, &req).await {
        Ok(result) => Json(result).into_response(),
        Err(message) => error_json(StatusCode::BAD_GATEWAY, &message),
    }
}

/// The create-channel spine: publish channel metadata -> read the on-chain creation fee ->
/// assemble pure `createChannel` calldata -> resolve the owner's wallet -> enqueue approval.
async fn run_create_channel(
    registry: &ProviderRegistry,
    principal_id: &str,
    req: &CreateChannelRequest,
) -> Result<Value, String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let name = req.name.trim();
    if name.is_empty() {
        return Err("a channel name is required".to_string());
    }
    let signer = req.creator_address.trim();
    if signer.is_empty() {
        return Err("creating a channel requires your wallet address (the signer)".to_string());
    }
    let scope = match req.scope.trim().to_ascii_lowercase().as_str() {
        "public" => CHANNEL_SCOPE_PUBLIC,
        "" | "private" => CHANNEL_SCOPE_PRIVATE,
        other => return Err(format!("unknown channel scope '{other}' (use public|private)")),
    };

    // 1) Publish minimal channel metadata to IPFS -> tokenURI (PC2 writes `ipfs://<cid>`).
    let channel_meta = json!({
        "version": "1.0",
        "name": name,
        "description": req.description.trim(),
    });
    let metadata_json = serde_json::to_string(&channel_meta)
        .map_err(|e| format!("serialize channel metadata: {e}"))?;
    let publish_dir = json!({
        "op": "publish",
        "kind": "directory",
        "files": [ { "path": "0.json", "data": b64.encode(metadata_json.as_bytes()) } ],
        "pin": true,
    });
    let metadata = provider_data(registry, "content", &publish_dir).await?;
    let meta_cid = metadata
        .get("cid")
        .and_then(Value::as_str)
        .ok_or("channel metadata publish returned no CID")?;
    let token_uri = format!("ipfs://{meta_cid}");

    // 2) Read the channelCreationFee on-chain (eth_call) — the payable `value`. token == 0x0
    //    means a native fee paid as msg.value; a non-zero token is an ERC-20 fee we can't
    //    cover via value (fail closed rather than send a tx that reverts).
    let value_wei = read_channel_creation_fee(registry).await?;

    // 3) Assemble the pure createChannel calldata.
    let assemble_req = json!({
        "op": "assemble_create_channel",
        "channel": {
            "selector": create_channel_selector(),
            "factory": channel_factory(),
            "channel_type": CHANNEL_TYPE_STANDARD,
            "scope": scope,
            "name": name,
            "token_uri": token_uri,
            "data_hex": EMPTY_CHANNEL_CONFIG_DATA,
            "value_wei": value_wei,
        }
    });
    let assembled = provider_data(registry, "chain", &assemble_req).await?;
    let to = assembled
        .get("to")
        .and_then(Value::as_str)
        .ok_or("chain assemble_create_channel returned no `to`")?
        .to_string();
    let data = assembled
        .get("data")
        .and_then(Value::as_str)
        .ok_or("chain assemble_create_channel returned no calldata")?
        .to_string();
    let value = assembled
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or("0x0")
        .to_string();

    // 4) Bind the owner's linked wallet + enqueue the MetaMask approval (owner deploys).
    let namespace = mint_chain_namespace();
    let account_id = resolve_owner_account(registry, principal_id, signer, &namespace).await?;
    // Let MetaMask estimate gas / gas price / nonce (PC2's createChannel sends only
    // to/data/value/chainId). createChannel also deploys a contract, so a fixed gas cap is
    // both fragile and unnecessary on the external-signer path.
    let intent = json!({
        "schema": "elastos.chain.unsigned_transaction_intent/v1",
        "transaction_type": "eip155_legacy",
        "from": signer,
        "to": to,
        "value": value,
        "data": data,
        "chain_id": mint_chain_id(),
        "requires_wallet_approval": true,
        "wallet_intent": "transaction_intent",
    });
    let sign_req = json!({
        "op": "request_signature",
        "principal_id": principal_id,
        "account_id": account_id,
        "chain_namespace": namespace,
        "intent": "transaction_intent",
        "capsule_id": CREATOR_APP,
        "resource": format!("elastos://chain/{}/broadcast_transaction", mint_network()),
        "reason": format!("Create dDRM channel \"{name}\" on {}", mint_network()),
        "payload": intent,
    });
    let approval = provider_data(registry, "wallet", &sign_req).await?;
    let request_id = approval
        .get("approval_request")
        .and_then(|r| r.get("request_id"))
        .and_then(Value::as_str)
        .ok_or("wallet returned no approval request id")?
        .to_string();

    Ok(json!({
        "schema": "elastos.creator.prepared-channel/v1",
        "name": name,
        "scope": scope,
        "token_uri": token_uri,
        "metadata_cid": meta_cid,
        "tx": { "to": to, "data": data, "value": value, "chain_id": mint_chain_id() },
        "channel_approval": { "request_id": request_id, "connector": "wallet-metamask" },
        "next": "approve_in_wallet",
    }))
}

// ── POST /api/apps/creator/prepare-trade-approval ───────────────────────────────
/// The trade-approval request body: the channel + the OWNER's signer address.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TradeApprovalRequest {
    #[serde(default)]
    channel: String,
    #[serde(default)]
    creator_address: String,
}

/// After the mint CONFIRMS on-chain, make the asset tradable by approving the channel's
/// authority gateway as an operator on the just-minted asset's operative contract — PC2's 2nd
/// mint tx (`setApprovalForAll(gateway, true)`, app.js:5104/5119). The runtime discovers the
/// operative + gateway FROM THE CHAIN (the operative is read from the `AssetCreated` log, which
/// only exists once the mint succeeded — so this is inherently confirmation-gated, #11),
/// assembles the pure approval calldata, and enqueues a wallet approval. The runtime never
/// signs; the OWNER completes the approval in the Wallet app.
pub async fn creator_prepare_trade_approval(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Result<Json<TradeApprovalRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let ctx =
        match require_home_launch_token_for_any_context(&state.data_dir, &headers, &[CREATOR_APP]) {
            Ok(ctx) => ctx,
            Err(_) => {
                return error_json(StatusCode::UNAUTHORIZED, "missing or invalid home launch token")
            }
        };
    let Some(registry) = state.provider_registry.as_ref() else {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "providers unavailable");
    };
    let Json(req) = match body {
        Ok(json) => json,
        Err(rejection) => return error_json(StatusCode::BAD_REQUEST, &rejection.body_text()),
    };
    match run_prepare_trade_approval(registry, &ctx.principal_id, &req).await {
        Ok(result) => Json(result).into_response(),
        Err(message) => error_json(StatusCode::BAD_GATEWAY, &message),
    }
}

/// The trade-approval spine: re-confirm the channel is the owner's (#11) -> ask the chain
/// provider to discover the just-minted operative + gateway and assemble the pure
/// `setApprovalForAll(gateway, true)` calldata -> if the gateway isn't already approved,
/// enqueue a wallet approval for the OWNER to sign. Mirrors PC2's confirmation-gated 2nd tx:
/// the operative comes from the mint's `AssetCreated` log, so a "mint_not_confirmed" error
/// just means the mint hasn't landed yet — the caller retries.
async fn run_prepare_trade_approval(
    registry: &ProviderRegistry,
    principal_id: &str,
    req: &TradeApprovalRequest,
) -> Result<Value, String> {
    let channel = req.channel.trim();
    if channel.is_empty() {
        return Err("a channel is required to enable trading".to_string());
    }
    let signer = req.creator_address.trim();
    if signer.is_empty() {
        return Err("enabling trading requires your wallet address (the signer)".to_string());
    }
    // Same fail-closed ownership gate as the mint: you may only approve on your own channel.
    confirm_channel_owned(registry, signer, channel).await?;

    let assembled = provider_data(
        registry,
        "chain",
        &json!({
            "op": "assemble_trade_approval",
            "network": mint_network(),
            "channel": channel,
            "creator": signer,
        }),
    )
    .await?;

    // Idempotent: the gateway is already an approved operator — nothing to sign.
    if assembled
        .get("already_approved")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(json!({
            "schema": "elastos.creator.trade-approval/v1",
            "already_approved": true,
            "operative": assembled.get("operative").cloned().unwrap_or(Value::Null),
            "gateway": assembled.get("gateway").cloned().unwrap_or(Value::Null),
            "next": "tradable",
        }));
    }

    let to = assembled
        .get("to")
        .and_then(Value::as_str)
        .ok_or("chain assemble_trade_approval returned no `to`")?
        .to_string();
    let data = assembled
        .get("data")
        .and_then(Value::as_str)
        .ok_or("chain assemble_trade_approval returned no calldata")?
        .to_string();
    let value = assembled
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or("0x0")
        .to_string();
    let operative = assembled
        .get("operative")
        .and_then(Value::as_str)
        .unwrap_or(&to)
        .to_string();

    let namespace = mint_chain_namespace();
    let account_id = resolve_owner_account(registry, principal_id, signer, &namespace).await?;
    // Let MetaMask estimate gas / gas price / nonce (same external-signer path as the mint).
    let intent = json!({
        "schema": "elastos.chain.unsigned_transaction_intent/v1",
        "transaction_type": "eip155_legacy",
        "from": signer,
        "to": to,
        "value": value,
        "data": data,
        "chain_id": mint_chain_id(),
        "requires_wallet_approval": true,
        "wallet_intent": "transaction_intent",
    });
    let sign_req = json!({
        "op": "request_signature",
        "principal_id": principal_id,
        "account_id": account_id,
        "chain_namespace": namespace,
        "intent": "transaction_intent",
        "capsule_id": CREATOR_APP,
        "resource": format!("elastos://chain/{}/broadcast_transaction", mint_network()),
        "reason": format!("Approve gateway to enable trading on {}", mint_network()),
        "payload": intent,
    });
    let approval = provider_data(registry, "wallet", &sign_req).await?;
    let request_id = approval
        .get("approval_request")
        .and_then(|r| r.get("request_id"))
        .and_then(Value::as_str)
        .ok_or("wallet returned no approval request id")?
        .to_string();

    Ok(json!({
        "schema": "elastos.creator.trade-approval/v1",
        "already_approved": false,
        "operative": operative,
        "gateway": assembled.get("gateway").cloned().unwrap_or(Value::Null),
        "tx": { "to": to, "data": data, "value": value, "chain_id": mint_chain_id() },
        "approval": { "request_id": request_id, "connector": "wallet-metamask" },
        "next": "approve_in_wallet",
    }))
}

/// Read `channelCreationFee() -> (uint256 fee, address token)` from CentralStorage and return
/// the payable `value` (hex). A native fee (token == zero) is paid as `msg.value`; a non-zero
/// ERC-20 token fee is refused (we won't assemble a tx that reverts for lack of allowance).
async fn read_channel_creation_fee(registry: &ProviderRegistry) -> Result<String, String> {
    if let Ok(forced) = std::env::var("ELASTOS_DDRM_CHANNEL_FEE_WEI") {
        let forced = forced.trim();
        if !forced.is_empty() {
            return Ok(forced.to_string());
        }
    }
    let call = json!({
        "op": "contract_call",
        "network": mint_network(),
        "to": central_storage(),
        "data": CHANNEL_CREATION_FEE_SELECTOR,
    });
    let res = provider_data(registry, "chain", &call).await?;
    let raw = res
        .get("result")
        .and_then(Value::as_str)
        .ok_or("channelCreationFee call returned no result")?;
    parse_channel_fee_result(raw)
}

/// Decode `channelCreationFee() -> (uint256 fee, address token)` into the payable `value`
/// (hex). A native fee (token == zero) is paid as `msg.value`; a non-zero ERC-20 token fee
/// is refused (fail closed — we won't assemble a tx that reverts for lack of allowance).
fn parse_channel_fee_result(raw: &str) -> Result<String, String> {
    let hex = raw.strip_prefix("0x").unwrap_or(raw);
    // (uint256 fee, address token): two 32-byte words.
    if hex.len() < 128 {
        return Err("channelCreationFee returned a short result".to_string());
    }
    let fee_word = &hex[0..64];
    let token_word = &hex[64..128];
    if token_word.chars().any(|c| c != '0') {
        return Err(
            "channel creation requires an ERC-20 fee — token-fee channels are not yet supported"
                .to_string(),
        );
    }
    let fee = fee_word.trim_start_matches('0');
    Ok(if fee.is_empty() {
        "0x0".to_string()
    } else {
        format!("0x{fee}")
    })
}

/// A failure annotated with the spine stage it happened at (so the UI lights the right dot).
struct StagedError {
    stage: &'static str,
    message: String,
}

fn stage_err(stage: &'static str, message: impl Into<String>) -> StagedError {
    StagedError { stage, message: message.into() }
}

/// The producer spine: escrow -> publish segment -> assemble envelope -> publish metadata
/// -> prepare unsigned mint. Returns the prepared mint for the user's wallet to sign.
async fn run_prepare_mint(
    registry: &ProviderRegistry,
    principal_id: &str,
    file_bytes: &[u8],
    meta: &MintMeta,
    nodes: &[QuorumNode],
) -> Result<Value, StagedError> {
    let b64 = base64::engine::general_purpose::STANDARD;

    // 1) escrow: mint CEK + CENC-encrypt + SHAMIR-split + seal a share to each node.
    let node_json: Vec<Value> = nodes
        .iter()
        .map(|n| {
            json!({
                "verifying_key_b64": n.verifying_key_b64,
                "recipient_pub_b64": n.recipient_pub_b64,
            })
        })
        .collect();
    let seal_req = json!({
        "op": "seal_inline_threshold",
        "plaintext_b64": b64.encode(file_bytes),
        "nodes": node_json,
    });
    let seal = provider_data(registry, "encrypt", &seal_req)
        .await
        .map_err(|e| stage_err("encrypt", e))?;
    // The raw CEK / plaintext must never come back.
    assert_no_raw_key_material(&seal).map_err(|e| stage_err("encrypt", e))?;

    let kid_hex = seal_str(&seal, "kid_hex").map_err(|e| stage_err("encrypt", e))?;
    let content_id = seal_str(&seal, "content_id_hex").map_err(|e| stage_err("encrypt", e))?;
    let segment_b64 = seal_str(&seal, "segment_b64").map_err(|e| stage_err("encrypt", e))?;
    let node_set_id_b64 = seal_str(&seal, "node_set_id_b64").map_err(|e| stage_err("encrypt", e))?;
    // The producer vk that signed each escrow seal. It MUST live in the envelope: the encrypt
    // producer key is minted fresh per process, so a later open (after the provider restarts)
    // can only authenticate the escrow against the persisted vk. Missing it => unrecoverable.
    let producer_vk_b64 =
        seal_str(&seal, "producer_verifying_key_b64").map_err(|e| stage_err("encrypt", e))?;
    let shares = seal
        .get("shares")
        .cloned()
        .filter(Value::is_array)
        .ok_or_else(|| stage_err("encrypt", "escrow response missing sealed shares"))?;

    // 2) publish the encrypted segment, get the asset CID.
    let asset_filename = sanitize_filename(&meta.file_name);
    let publish_file = json!({
        "op": "publish",
        "kind": "file",
        "data": segment_b64,
        "filename": asset_filename,
        "pin": true,
    });
    let asset = provider_data(registry, "content", &publish_file)
        .await
        .map_err(|e| stage_err("publish", e))?;
    let asset_cid = asset
        .get("cid")
        .and_then(Value::as_str)
        .ok_or_else(|| stage_err("publish", "content publish returned no CID"))?
        .to_string();

    // 3) assemble the metadata envelope — the dKMS escrow descriptor takes Lit's place.
    let envelope = build_metadata_envelope(BuildEnvelope {
        kid_hex: &kid_hex,
        asset_cid: &asset_cid,
        node_set_id_b64: &node_set_id_b64,
        shares: &shares,
        producer_vk_b64: &producer_vk_b64,
        meta,
        creator_principal: principal_id,
    });
    let protections = envelope["asset"]["protections"].clone();

    // 4) publish metadata.json as a directory, get the metadata CID.
    let metadata_json = serde_json::to_string(&envelope)
        .map_err(|e| stage_err("publish", format!("serialize envelope: {e}")))?;
    let publish_dir = json!({
        "op": "publish",
        "kind": "directory",
        "files": [ { "path": "metadata.json", "data": b64.encode(metadata_json.as_bytes()) } ],
        "pin": true,
    });
    let metadata = provider_data(registry, "content", &publish_dir)
        .await
        .map_err(|e| stage_err("publish", e))?;
    let metadata_cid = metadata
        .get("cid")
        .and_then(Value::as_str)
        .ok_or_else(|| stage_err("publish", "metadata publish returned no CID"))?
        .to_string();

    // 5-8) prepare the unsigned mint, assemble Base calldata, bind to the owner's wallet,
    //       and enqueue the MetaMask approval. Shared verbatim with the media path.
    finalize_mint(
        registry,
        principal_id,
        meta,
        MintTail {
            kid_hex: &kid_hex,
            content_id: &content_id,
            asset_cid: &asset_cid,
            metadata_cid: &metadata_cid,
            protections: &protections,
        },
    )
    .await
}

/// The non-secret outputs of the producer spine that the mint tail binds into the
/// unsigned mint + the prepared-mint response (shared by the object + media paths).
struct MintTail<'a> {
    kid_hex: &'a str,
    content_id: &'a str,
    /// The on-chain/IPFS asset CID: a single segment CID (object) or the DASH directory CID (media).
    asset_cid: &'a str,
    metadata_cid: &'a str,
    protections: &'a Value,
}

/// The shared mint tail: `publish prepare_publish` (unsigned mint, contentId == bytes16
/// KID, pricing for paid) -> `chain assemble_mint` (real Base calldata) -> resolve the
/// OWNER's linked wallet account -> `wallet request_signature` (MetaMask approval). The
/// runtime signs NOTHING; the owner completes the mint, so the owner is the on-chain creator.
async fn finalize_mint(
    registry: &ProviderRegistry,
    principal_id: &str,
    meta: &MintMeta,
    tail: MintTail<'_>,
) -> Result<Value, StagedError> {
    let kid_hex = tail.kid_hex;
    let metadata_cid = tail.metadata_cid;

    let channel = meta.channel.trim();
    if channel.is_empty() {
        return Err(stage_err(
            "publish",
            "a channel is required — select one of your channels (or create one) first",
        ));
    }
    // Fail-closed ownership gate (#11): you may only mint into a channel YOU created. The
    // authoritative on-chain proof is the factory's `ChannelCreated` event, whose indexed
    // `creator` topic must equal the signer — exactly what `list_channels` filters on. So a
    // channel is "yours" iff it appears in your on-chain-discovered set. This blocks minting
    // into another creator's channel even if its address is entered manually.
    let signer_for_channel = meta.creator_address.trim();
    if signer_for_channel.is_empty() {
        return Err(stage_err(
            "publish",
            "minting requires your wallet address (the signer) — connect your wallet",
        ));
    }
    confirm_channel_owned(registry, signer_for_channel, channel)
        .await
        .map_err(|e| stage_err("publish", &e))?;
    let mut publish_req = json!({
        "op": "prepare_publish",
        "request": {
            "schema": "elastos.publish.request/v1",
            "request_id": format!("creator:{kid_hex}"),
            "kid_hex": kid_hex,
            "metadata_cid": metadata_cid,
            "channel_address": channel,
            "op_type": op_type_for(meta),
        }
    });
    if is_paid(meta) {
        let price_wei = to_wei(&meta.price, &meta.currency)
            .map_err(|e| stage_err("publish", e))?;
        let creator = meta.creator_address.trim();
        if creator.is_empty() {
            return Err(stage_err(
                "publish",
                "a paid mint needs the creator payout address (your wallet) — connect a wallet",
            ));
        }
        let req = publish_req["request"].as_object_mut().unwrap();
        req.insert("price_wei".into(), json!(price_wei));
        req.insert("creator_address".into(), json!(creator));
        if !meta.currency.eq_ignore_ascii_case("ELA")
            && !meta.currency.eq_ignore_ascii_case("ETH")
            && !meta.currency.trim().is_empty()
        {
            // Native currencies omit a token address; named ERC-20s would carry one.
            // (Resolving symbol -> token address is a follow-on; native works today.)
        }
    }
    let prepared = provider_data(registry, "publish", &publish_req)
        .await
        .map_err(|e| stage_err("sign", e))?;
    let unsigned_mint = prepared
        .get("unsigned_mint")
        .cloned()
        .ok_or_else(|| stage_err("sign", "publish provider returned no unsigned mint"))?;

    // The publish provider's `unsigned_mint` is a RICHER descriptor (schema, chain_id,
    // function, op_type, fee_source, signed) than the chain capability's strict ABI input.
    // chain-provider::assemble_mint fails closed on unknown fields (#11), so project the
    // descriptor onto exactly the fields it accepts and add the configured selector — the
    // orchestrator owns this translation, keeping both provider contracts strict (#5, #10).
    let um = unsigned_mint
        .as_object()
        .ok_or_else(|| stage_err("sign", "publish provider returned a non-object unsigned mint"))?;
    let mut mint = serde_json::Map::new();
    mint.insert("selector".into(), json!(mint_selector()));
    for key in [
        "to",
        "token_uri",
        "op_type_code",
        "content_id",
        "value_wei",
        "op_raw",
        "sell",
    ] {
        if let Some(value) = um.get(key) {
            mint.insert(key.to_string(), value.clone());
        }
    }
    let assemble_req = json!({ "op": "assemble_mint", "mint": Value::Object(mint) });
    let assembled = provider_data(registry, "chain", &assemble_req)
        .await
        .map_err(|e| stage_err("sign", e))?;
    let to = assembled
        .get("to")
        .and_then(Value::as_str)
        .ok_or_else(|| stage_err("sign", "chain assemble_mint returned no `to`"))?
        .to_string();
    let data = assembled
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| stage_err("sign", "chain assemble_mint returned no calldata"))?
        .to_string();
    let value = assembled
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or("0x0")
        .to_string();

    let signer = meta.creator_address.trim();
    if signer.is_empty() {
        return Err(stage_err(
            "sign",
            "minting requires the creator wallet address (the signer) — connect your wallet",
        ));
    }
    let namespace = mint_chain_namespace();
    let account_id = resolve_owner_account(registry, principal_id, signer, &namespace)
        .await
        .map_err(|e| stage_err("sign", e))?;

    // No nonce / gas_price / gas_limit: the mint deploys an operative contract (~1.5M gas),
    // so a fixed cap risks out-of-gas, and a fixed price inflates the fee on Base. Like PC2's
    // EOA mint we hand the wallet only to/data/value/chainId and let MetaMask estimate gas,
    // gas price, and nonce — the canonical external-signer path.
    let intent = json!({
        "schema": "elastos.chain.unsigned_transaction_intent/v1",
        "transaction_type": "eip155_legacy",
        "from": signer,
        "to": to,
        "value": value,
        "data": data,
        "chain_id": mint_chain_id(),
        "requires_wallet_approval": true,
        "wallet_intent": "transaction_intent",
    });
    let title = meta.title.trim();
    let sign_req = json!({
        "op": "request_signature",
        "principal_id": principal_id,
        "account_id": account_id,
        "chain_namespace": namespace,
        "intent": "transaction_intent",
        "capsule_id": CREATOR_APP,
        "resource": format!("elastos://chain/{}/broadcast_transaction", mint_network()),
        "reason": format!("Mint dDRM asset \"{title}\" on {}", mint_network()),
        "payload": intent,
    });
    let approval = provider_data(registry, "wallet", &sign_req)
        .await
        .map_err(|e| stage_err("sign", e))?;
    let request_id = approval
        .get("approval_request")
        .and_then(|r| r.get("request_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| stage_err("sign", "wallet returned no approval request id"))?
        .to_string();

    Ok(json!({
        "schema": "elastos.creator.prepared-mint/v1",
        "content_id": tail.content_id,
        "kid": kid_hex,
        "asset_cid": tail.asset_cid,
        "metadata_cid": metadata_cid,
        "protections": tail.protections,
        "unsigned_mint": unsigned_mint,
        // The exact transaction the OWNER will sign — surfaced for offline inspection.
        "tx": { "to": to, "data": data, "value": value, "chain_id": mint_chain_id() },
        // The OWNER completes the mint by approving this request in the Wallet app.
        "mint_approval": { "request_id": request_id, "connector": "wallet-metamask" },
        "next": "approve_in_wallet",
    }))
}

/// The MEDIA producer spine: DASH-package (per-track inits + plaintext segments + MPD) ->
/// CENC every fragment under ONE asset CEK (single default_KID) + escrow to the quorum ->
/// publish the DASH directory (plaintext inits + ENCRYPTED segments + manifest.mpd) -> media
/// envelope (dir CID + default_KID + dKMS escrow, swapped in where PC2 wrote Lit) -> shared
/// mint tail. The CEK never leaves the encrypt-provider boundary; only sealed shares return.
async fn run_prepare_mint_media(
    registry: &ProviderRegistry,
    principal_id: &str,
    file_bytes: &[u8],
    meta: &MintMeta,
    nodes: &[QuorumNode],
) -> Result<Value, StagedError> {
    let b64 = base64::engine::general_purpose::STANDARD;

    // 1) DASH-package the source: per-track standalone inits + PLAINTEXT segments + manifest.
    let pkg_req = json!({
        "op": "package_dash",
        "content_b64": b64.encode(file_bytes),
        "filename": meta.file_name,
    });
    let pkg = provider_data(registry, "media", &pkg_req)
        .await
        .map_err(|e| stage_err("encrypt", e))?;
    let manifest = pkg
        .get("mpd")
        .and_then(Value::as_str)
        .ok_or_else(|| stage_err("encrypt", "media package returned no MPD"))?
        .to_string();
    let tracks = pkg
        .get("tracks")
        .and_then(Value::as_array)
        .ok_or_else(|| stage_err("encrypt", "media package returned no tracks"))?;

    // Flatten every media segment across tracks IN ORDER (so the single-CEK CENC counter is
    // continuous), recording each segment's directory path + the plaintext per-track inits.
    let mut all_segments: Vec<String> = Vec::new();
    let mut seg_paths: Vec<String> = Vec::new(); // parallel to all_segments
    let mut init_files: Vec<(String, String)> = Vec::new(); // (path, plaintext b64)
    let mut first_init_b64: Option<String> = None;
    for t in tracks {
        let init_path = t
            .get("init_path")
            .and_then(Value::as_str)
            .ok_or_else(|| stage_err("encrypt", "track missing init_path"))?;
        let init_b64 = t
            .get("init_b64")
            .and_then(Value::as_str)
            .ok_or_else(|| stage_err("encrypt", "track missing init_b64"))?;
        if first_init_b64.is_none() {
            first_init_b64 = Some(init_b64.to_string());
        }
        init_files.push((init_path.to_string(), init_b64.to_string()));
        let segs = t
            .get("segments")
            .and_then(Value::as_array)
            .ok_or_else(|| stage_err("encrypt", "track missing segments"))?;
        for s in segs {
            let path = s
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| stage_err("encrypt", "segment missing path"))?;
            let data = s
                .get("b64")
                .and_then(Value::as_str)
                .ok_or_else(|| stage_err("encrypt", "segment missing b64"))?;
            seg_paths.push(path.to_string());
            all_segments.push(data.to_string());
        }
    }
    if all_segments.is_empty() {
        return Err(stage_err("encrypt", "media package produced no segments"));
    }

    // 2) CENC every fragment under ONE asset CEK (single default_KID); escrow to the quorum.
    let node_json: Vec<Value> = nodes
        .iter()
        .map(|n| {
            json!({
                "verifying_key_b64": n.verifying_key_b64,
                "recipient_pub_b64": n.recipient_pub_b64,
            })
        })
        .collect();
    let seal_req = json!({
        "op": "seal_segments_threshold",
        "segments_b64": all_segments,
        "init_b64": first_init_b64,
        "nodes": node_json,
    });
    let seal = provider_data(registry, "encrypt", &seal_req)
        .await
        .map_err(|e| stage_err("encrypt", e))?;
    assert_no_raw_key_material(&seal).map_err(|e| stage_err("encrypt", e))?;
    let kid_hex = seal_str(&seal, "kid_hex").map_err(|e| stage_err("encrypt", e))?;
    let content_id = seal_str(&seal, "content_id_hex").map_err(|e| stage_err("encrypt", e))?;
    let node_set_id_b64 = seal_str(&seal, "node_set_id_b64").map_err(|e| stage_err("encrypt", e))?;
    // See the object path: the per-process producer vk MUST be persisted or the asset is
    // unrecoverable after the encrypt provider restarts.
    let producer_vk_b64 =
        seal_str(&seal, "producer_verifying_key_b64").map_err(|e| stage_err("encrypt", e))?;
    let shares = seal
        .get("shares")
        .cloned()
        .filter(Value::is_array)
        .ok_or_else(|| stage_err("encrypt", "escrow response missing sealed shares"))?;
    let enc_segments = seal
        .get("segments_b64")
        .and_then(Value::as_array)
        .ok_or_else(|| stage_err("encrypt", "escrow response missing encrypted segments"))?;
    if enc_segments.len() != all_segments.len() {
        return Err(stage_err("encrypt", "encrypted segment count mismatch"));
    }

    // 3) assemble the DASH directory: plaintext per-track inits + ENCRYPTED segments (at their
    //    MPD paths) + the manifest. Inits are NOT encrypted (CENC encrypts media fragments only).
    let mut files: Vec<Value> = Vec::with_capacity(init_files.len() + enc_segments.len() + 1);
    for (path, data) in &init_files {
        files.push(json!({ "path": path, "data": data }));
    }
    for (i, enc) in enc_segments.iter().enumerate() {
        let data = enc
            .as_str()
            .ok_or_else(|| stage_err("encrypt", "encrypted segment not a string"))?;
        files.push(json!({ "path": seg_paths[i], "data": data }));
    }
    files.push(json!({ "path": "manifest.mpd", "data": b64.encode(manifest.as_bytes()) }));

    // 4) publish the whole DASH directory -> one dir CID (the asset CID).
    let publish_dir = json!({ "op": "publish", "kind": "directory", "files": files, "pin": true });
    let asset = provider_data(registry, "content", &publish_dir)
        .await
        .map_err(|e| stage_err("publish", e))?;
    let dir_cid = asset
        .get("cid")
        .and_then(Value::as_str)
        .ok_or_else(|| stage_err("publish", "content publish returned no CID"))?
        .to_string();

    // 5) media metadata envelope (dir CID + default_KID + dKMS escrow).
    let envelope = build_media_envelope(MediaEnvelope {
        kid_hex: &kid_hex,
        dir_cid: &dir_cid,
        manifest_path: "manifest.mpd",
        node_set_id_b64: &node_set_id_b64,
        shares: &shares,
        producer_vk_b64: &producer_vk_b64,
        tracks,
        meta,
        creator_principal: principal_id,
    });
    let protections = envelope["asset"]["protections"].clone();
    let metadata_json = serde_json::to_string(&envelope)
        .map_err(|e| stage_err("publish", format!("serialize envelope: {e}")))?;
    let publish_meta = json!({
        "op": "publish",
        "kind": "directory",
        "files": [ { "path": "metadata.json", "data": b64.encode(metadata_json.as_bytes()) } ],
        "pin": true,
    });
    let metadata = provider_data(registry, "content", &publish_meta)
        .await
        .map_err(|e| stage_err("publish", e))?;
    let metadata_cid = metadata
        .get("cid")
        .and_then(Value::as_str)
        .ok_or_else(|| stage_err("publish", "metadata publish returned no CID"))?
        .to_string();

    // 6) shared mint tail — the asset CID is the DASH directory CID.
    finalize_mint(
        registry,
        principal_id,
        meta,
        MintTail {
            kid_hex: &kid_hex,
            content_id: &content_id,
            asset_cid: &dir_cid,
            metadata_cid: &metadata_cid,
            protections: &protections,
        },
    )
    .await
}

struct MediaEnvelope<'a> {
    kid_hex: &'a str,
    dir_cid: &'a str,
    manifest_path: &'a str,
    node_set_id_b64: &'a str,
    shares: &'a Value,
    /// The producer verifying key that signed each escrow seal (PUBLIC). Persisted so a later
    /// consumer open can authenticate the quorum escrow recovery against it.
    producer_vk_b64: &'a str,
    tracks: &'a [Value],
    meta: &'a MintMeta,
    creator_principal: &'a str,
}

/// Build the public DASH asset envelope. The CEK-custody block (`asset.protections[0]`) is
/// identical to the object path — the dKMS escrow descriptor. The `media` block carries the
/// DASH manifest path, the single `defaultKID`, and a per-track summary (PC2's media block).
fn build_media_envelope(b: MediaEnvelope) -> Value {
    let track_summaries: Vec<Value> = b
        .tracks
        .iter()
        .map(|t| {
            json!({
                "kind": t.get("kind"),
                "track_id": t.get("track_id"),
                "codec": t.get("codec"),
                "bandwidth": t.get("bandwidth"),
                "width": t.get("width"),
                "height": t.get("height"),
            })
        })
        .collect();
    json!({
        "schema": ENVELOPE_SCHEMA,
        "kid": b.kid_hex,
        "asset": {
            "title": b.meta.title.trim(),
            "description": b.meta.description.trim(),
            "kid": b.kid_hex,
            "mimeType": if b.meta.mime.is_empty() { "video/mp4" } else { b.meta.mime.as_str() },
            "assetCid": b.dir_cid,
            "creatorPrincipal": b.creator_principal,
            "protections": [{
                "algorithm": "aes-128",
                "protectionType": THRESHOLD_PROTECTION_TYPE,
                "scheme": THRESHOLD_SCHEME,
                "chain": "base",
                "node_set_id_b64": b.node_set_id_b64,
                "producer_verifying_key_b64": b.producer_vk_b64,
                "shares": b.shares.clone(),
            }],
        },
        "media": {
            "mediaType": "dash",
            "manifestPath": b.manifest_path,
            "defaultKID": b.kid_hex,
            "tracks": track_summaries,
        },
    })
}

/// Confirm on-chain that `channel` was created by `signer` — the fail-closed ownership gate
/// for minting. Ownership proof is the factory's `ChannelCreated` event (indexed `creator`),
/// which `list_channels` scans, so "owned" == "present in the creator's discovered set". A
/// not-found result distinguishes "still indexing" (retryable) from "not yours" (hard refusal),
/// so the operator gets an actionable message instead of a silent failure.
async fn confirm_channel_owned(
    registry: &ProviderRegistry,
    signer: &str,
    channel: &str,
) -> Result<(), String> {
    let listed = provider_data(
        registry,
        "chain",
        &json!({
            "op": "list_channels",
            "network": mint_network(),
            "factory": channel_factory(),
            "creator": signer,
        }),
    )
    .await?;
    let owned = listed
        .get("channels")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter().any(|c| {
                c.get("address")
                    .and_then(Value::as_str)
                    .map(|addr| addr.eq_ignore_ascii_case(channel))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if owned {
        return Ok(());
    }
    let indexing = listed
        .get("indexing")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if indexing {
        Err(format!(
            "couldn't confirm channel {channel} belongs to your wallet yet — channel discovery is still indexing; wait a few seconds and mint again"
        ))
    } else {
        Err(format!(
            "channel {channel} was not created by the connected wallet — you can only mint into your own channel"
        ))
    }
}

/// Find the OWNER's linked EVM account that can sign the mint, matched by signer address.
/// The mint is signed by the OWNER's wallet (not the runtime). An EVM address is the same
/// key on every `eip155:*` chain, so an account linked on ANY EVM chain (e.g. `eip155:1`)
/// can sign the Base mint — the wallet provider accepts it (`chain_namespaces_compatible`)
/// and the metamask connector switches to Base at sign time. We prefer an exact mint-chain
/// match, then fall back to any compatible EVM account. Returns the `account_id` to bind.
async fn resolve_owner_account(
    registry: &ProviderRegistry,
    principal_id: &str,
    signer: &str,
    namespace: &str,
) -> Result<String, String> {
    let accounts = provider_data(
        registry,
        "wallet",
        &json!({ "op": "accounts", "principal_id": principal_id }),
    )
    .await?;
    let list = accounts
        .get("accounts")
        .and_then(Value::as_array)
        .ok_or("wallet returned no accounts")?;
    let address_matches = |a: &&Value| {
        a.get("address")
            .and_then(Value::as_str)
            .map(|addr| addr.eq_ignore_ascii_case(signer))
            .unwrap_or(false)
    };
    let exact = list.iter().find(|a| {
        a.get("chain_namespace").and_then(Value::as_str) == Some(namespace) && address_matches(a)
    });
    let matched = exact.or_else(|| {
        list.iter().find(|a| {
            a.get("chain_namespace")
                .and_then(Value::as_str)
                .is_some_and(chain_namespace_can_sign_mint)
                && address_matches(a)
        })
    });
    match matched {
        Some(a) => a
            .get("account_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "matched wallet account has no account_id".to_string()),
        None => Err(format!(
            "wallet {signer} is not linked to this principal — link it in the Wallet app first"
        )),
    }
}

struct BuildEnvelope<'a> {
    kid_hex: &'a str,
    asset_cid: &'a str,
    node_set_id_b64: &'a str,
    shares: &'a Value,
    /// The producer verifying key that signed each escrow seal (PUBLIC). Persisted so a
    /// later consumer open can authenticate the quorum escrow recovery against it.
    producer_vk_b64: &'a str,
    meta: &'a MintMeta,
    creator_principal: &'a str,
}

/// Build the public asset metadata envelope. The CEK-custody block lives in
/// `asset.protections[0]` — the dKMS escrow descriptor (node-set pin + per-node SEALED
/// shares), placed EXACTLY where PC2's creator wrote `litCiphertext`/`litBackend`.
fn build_metadata_envelope(b: BuildEnvelope) -> Value {
    json!({
        "schema": ENVELOPE_SCHEMA,
        "kid": b.kid_hex,
        "asset": {
            "title": b.meta.title.trim(),
            "description": b.meta.description.trim(),
            "kid": b.kid_hex,
            "mimeType": if b.meta.mime.is_empty() { "application/octet-stream" } else { b.meta.mime.as_str() },
            "assetCid": b.asset_cid,
            "creatorPrincipal": b.creator_principal,
            "protections": [{
                "algorithm": "aes-128",
                "protectionType": THRESHOLD_PROTECTION_TYPE,
                "scheme": THRESHOLD_SCHEME,
                "chain": "base",
                // The node-set pin the open must match (detects a node swap).
                "node_set_id_b64": b.node_set_id_b64,
                // The producer vk that signed each escrow seal — the recovery boundary
                // authenticates the quorum escrow under it. Persisted here because the
                // encrypt producer key is per-process; without it the asset is unrecoverable.
                "producer_verifying_key_b64": b.producer_vk_b64,
                // Per-node SEALED indexed shares — public escrow ciphertext the quorum
                // (and only the quorum, under rights) can unwrap. The dKMS analogue of
                // PC2's public `litCiphertext`.
                "shares": b.shares.clone(),
            }],
        },
        "media": { "mediaType": "object" },
    })
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Send a request to a provider and return its `data` object, mapping the uniform
/// `{status:"error", message}` envelope into an error string.
async fn provider_data(
    registry: &ProviderRegistry,
    scheme: &str,
    request: &Value,
) -> Result<Value, String> {
    let response = registry
        .send_raw(scheme, request)
        .await
        .map_err(|e| format!("{scheme} provider unavailable: {e}"))?;
    if response.get("status").and_then(Value::as_str) == Some("error") {
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("provider returned an error");
        return Err(format!("{scheme}: {message}"));
    }
    response
        .get("data")
        .cloned()
        .ok_or_else(|| format!("{scheme} response missing data"))
}

fn seal_str(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("escrow response missing {key}"))
}

fn is_paid(meta: &MintMeta) -> bool {
    !price_is_zero(&meta.price)
}

fn price_is_zero(price: &str) -> bool {
    let p = price.trim();
    p.is_empty() || p.chars().all(|c| c == '0' || c == '.' )
}

fn op_type_for(meta: &MintMeta) -> &'static str {
    if is_paid(meta) {
        "buy_once"
    } else {
        "free"
    }
}

/// Convert a human decimal price + currency into the smallest-unit (wei-like) decimal
/// string the publish provider expects. Native (ELA/ETH) and unspecified default to 18
/// decimals; USDC is 6. No floats — scale the decimal string exactly.
fn to_wei(price: &str, currency: &str) -> Result<String, String> {
    let decimals: u32 = match currency.trim().to_ascii_uppercase().as_str() {
        "USDC" => 6,
        _ => 18,
    };
    let price = price.trim();
    if price.is_empty() {
        return Err("price is empty".into());
    }
    let (int_part, frac_part) = match price.split_once('.') {
        Some((i, f)) => (i, f),
        None => (price, ""),
    };
    if !int_part.chars().all(|c| c.is_ascii_digit())
        || !frac_part.chars().all(|c| c.is_ascii_digit())
    {
        return Err(format!("price '{price}' is not a valid decimal"));
    }
    if frac_part.len() > decimals as usize {
        return Err(format!(
            "price '{price}' has more decimals than {currency} supports ({decimals})"
        ));
    }
    let mut digits = String::new();
    digits.push_str(int_part.trim_start_matches('0'));
    digits.push_str(frac_part);
    // Pad the fractional shortfall with zeros (scale up to `decimals`).
    for _ in 0..(decimals as usize - frac_part.len()) {
        digits.push('0');
    }
    let trimmed = digits.trim_start_matches('0');
    Ok(if trimmed.is_empty() { "0".to_string() } else { trimmed.to_string() })
}

/// Persist a freshly-prepared mint into the creator's Library so it lists at the correct
/// location and opens on double-click — the runtime analogue of PC2 indexing a creator's own
/// mint into their catalog. Two objects are written under the principal's `Documents`:
///   1. the asset itself — the openable owned object the protected viewer renders, and
///   2. a PC2-style `.ddrm` capsule carrying the dKMS escrow (`protections[0]`) + the on-chain
///      identities (contentId/KID, asset+metadata CIDs), i.e. exactly the fields the proven
///      quorum-open phase reloads from disk to recover the CEK 2-of-3.
///
/// Best-effort: a persistence failure is logged but NEVER fails the mint (the tx is already
/// prepared). Persisting at prepare-time (before the on-chain confirm) is a deliberate
/// simplification so the asset is visible immediately; a chain-indexed variant can supersede it.
fn persist_minted_asset_to_library(
    data_dir: &FsPath,
    principal_id: &str,
    meta: &MintMeta,
    file_bytes: &[u8],
    prepared: &Value,
) {
    let root = crate::auth::principal_localhost_root(principal_id);
    let base = {
        let name = if !meta.file_name.trim().is_empty() {
            meta.file_name.trim()
        } else {
            meta.title.trim()
        };
        sanitize_filename(name)
    };
    let mime = Some(meta.mime.trim()).filter(|m| !m.is_empty());

    // 1) the openable asset object (encrypted at rest by the principal-root layer).
    let asset_uri = format!("{root}/Documents/{base}");
    if let Err(err) = crate::library::handle_library_upload_bytes(
        data_dir, principal_id, &asset_uri, mime, None, file_bytes,
    ) {
        tracing::warn!("mint persist: could not write owned asset {asset_uri}: {err}");
        return;
    }

    // 2) the PC2-style `.ddrm` capsule carrying the dKMS escrow + on-chain identities.
    let capsule = json!({
        "schema": "elastos.ddrm.capsule/v1",
        "title": meta.title.trim(),
        "description": meta.description.trim(),
        "mime": meta.mime.trim(),
        "is_media": meta.is_media,
        "channel": meta.channel.trim(),
        "creator_address": meta.creator_address.trim(),
        "content_id": prepared.get("content_id").cloned().unwrap_or(Value::Null),
        "kid": prepared.get("kid").cloned().unwrap_or(Value::Null),
        "asset_cid": prepared.get("asset_cid").cloned().unwrap_or(Value::Null),
        "metadata_cid": prepared.get("metadata_cid").cloned().unwrap_or(Value::Null),
        // The dKMS escrow descriptor (scheme, node_set_id_b64, producer_verifying_key_b64,
        // shares[]) — the exact fields the proven quorum-open phase reloads from disk.
        "protections": prepared.get("protections").cloned().unwrap_or(Value::Null),
        "asset_object_uri": asset_uri,
    });
    let capsule_uri = format!("{root}/Documents/{base}.ddrm");
    let capsule_bytes = serde_json::to_vec_pretty(&capsule).unwrap_or_default();
    if let Err(err) = crate::library::handle_library_upload_bytes(
        data_dir,
        principal_id,
        &capsule_uri,
        Some("application/x-ddrm"),
        None,
        &capsule_bytes,
    ) {
        tracing::warn!("mint persist: could not write .ddrm capsule {capsule_uri}: {err}");
    }
    tracing::info!(
        "mint persist: wrote owned asset + .ddrm capsule for \"{}\" under {root}/Documents",
        meta.title.trim()
    );
}

fn sanitize_filename(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return "asset.bin".to_string();
    }
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if cleaned.is_empty() { "asset.bin".to_string() } else { cleaned }
}

/// Load the PUBLIC-ONLY quorum descriptor (`threshold.nodes[]`: verifying + recipient keys).
/// Refuses a descriptor that carries a master seed — the secret must stay in the node.
fn load_quorum_descriptor(data_dir: &FsPath) -> Result<Vec<QuorumNode>, String> {
    let path = std::env::var(QUORUM_DESCRIPTOR_ENV)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| data_dir.join("dkms").join("quorum.json"));
    let bytes = std::fs::read(&path).map_err(|_| {
        format!(
            "no dKMS quorum descriptor at {} — provision the PUBLIC-ONLY descriptor (or set {})",
            path.display(),
            QUORUM_DESCRIPTOR_ENV
        )
    })?;
    parse_quorum_descriptor(&bytes)
}

/// Parse + validate a PUBLIC-ONLY quorum descriptor. Pure (testable without disk).
fn parse_quorum_descriptor(bytes: &[u8]) -> Result<Vec<QuorumNode>, String> {
    let desc: Value = serde_json::from_slice(bytes).map_err(|e| format!("descriptor is not valid JSON: {e}"))?;
    if has_secret_material(&desc) {
        return Err("descriptor carries secret material — it must be PUBLIC-ONLY".into());
    }
    let nodes_v = desc
        .get("threshold")
        .and_then(|t| t.get("nodes"))
        .and_then(Value::as_array)
        .ok_or("descriptor has no threshold.nodes array")?;
    if nodes_v.len() != 3 {
        return Err(format!(
            "the Create portal requires a 3-node 2-of-3 quorum; descriptor has {}",
            nodes_v.len()
        ));
    }
    let mut nodes = Vec::with_capacity(3);
    for (i, n) in nodes_v.iter().enumerate() {
        let verifying_key_b64 = n
            .get("verifying_key_b64")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("node {i} missing verifying_key_b64"))?
            .to_string();
        let recipient_pub_b64 = n
            .get("recipient_pub_b64")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("node {i} missing recipient_pub_b64"))?
            .to_string();
        nodes.push(QuorumNode { verifying_key_b64, recipient_pub_b64 });
    }
    Ok(nodes)
}

fn has_secret_material(value: &Value) -> bool {
    let mut stack = vec![value];
    while let Some(v) = stack.pop() {
        match v {
            Value::Object(map) => {
                for (k, child) in map {
                    let lk = k.to_ascii_lowercase();
                    if lk.contains("master_seed") || lk.contains("private") || lk == "seed" {
                        return true;
                    }
                    stack.push(child);
                }
            }
            Value::Array(items) => stack.extend(items.iter()),
            _ => {}
        }
    }
    false
}

/// Recursively refuse a value that carries any raw-secret field.
fn assert_no_raw_key_material(value: &Value) -> Result<(), String> {
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    if FORBIDDEN_RAW_KEY_FIELDS
                        .iter()
                        .any(|forbidden| key.eq_ignore_ascii_case(forbidden))
                    {
                        return Err(format!("forbidden raw key field: {key}"));
                    }
                    stack.push(child);
                }
            }
            Value::Array(items) => stack.extend(items.iter()),
            _ => {}
        }
    }
    Ok(())
}

fn error_json(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn staged_error(status: StatusCode, stage: &str, message: &str) -> Response {
    (status, Json(json!({ "stage": stage, "error": message }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(node_count: usize, with_seed: bool) -> Vec<u8> {
        let mut nodes = Vec::new();
        for i in 0..node_count {
            nodes.push(json!({
                "verifying_key_b64": format!("vk{i}"),
                "recipient_pub_b64": format!("rk{i}"),
            }));
        }
        let mut desc = json!({ "threshold": { "nodes": nodes } });
        if with_seed {
            desc["threshold"]["master_seed_b64"] = json!("c2VjcmV0");
        }
        serde_json::to_vec(&desc).unwrap()
    }

    #[test]
    fn parses_a_public_three_node_descriptor() {
        let nodes = parse_quorum_descriptor(&descriptor(3, false)).expect("should parse");
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].verifying_key_b64, "vk0");
        assert_eq!(nodes[2].recipient_pub_b64, "rk2");
    }

    #[test]
    fn refuses_a_descriptor_carrying_secret_material() {
        let err = parse_quorum_descriptor(&descriptor(3, true)).unwrap_err();
        assert!(err.contains("PUBLIC-ONLY"), "got: {err}");
    }

    #[test]
    fn refuses_a_quorum_that_is_not_three_nodes() {
        assert!(parse_quorum_descriptor(&descriptor(2, false)).is_err());
        assert!(parse_quorum_descriptor(&descriptor(4, false)).is_err());
    }

    #[test]
    fn op_type_follows_price() {
        let free = MintMeta { price: "0".into(), ..Default::default() };
        assert_eq!(op_type_for(&free), "free");
        assert!(!is_paid(&free));
        let empty = MintMeta { price: "".into(), ..Default::default() };
        assert_eq!(op_type_for(&empty), "free");
        let paid = MintMeta { price: "1.5".into(), ..Default::default() };
        assert_eq!(op_type_for(&paid), "buy_once");
        assert!(is_paid(&paid));
    }

    #[test]
    fn price_to_wei_scales_exactly_no_floats() {
        assert_eq!(to_wei("1", "ELA").unwrap(), "1000000000000000000");
        assert_eq!(to_wei("1.5", "ETH").unwrap(), "1500000000000000000");
        assert_eq!(to_wei("0.000001", "ELA").unwrap(), "1000000000000");
        assert_eq!(to_wei("1", "USDC").unwrap(), "1000000");
        assert_eq!(to_wei("2.5", "USDC").unwrap(), "2500000");
        // More fractional digits than the currency supports is refused.
        assert!(to_wei("1.0000001", "USDC").is_err());
        assert!(to_wei("abc", "ELA").is_err());
    }

    #[test]
    fn envelope_puts_the_dkms_escrow_where_lit_used_to_be() {
        let shares = json!([
            { "x": 1, "verifying_key_b64": "vk0", "wrapped_share_b64": "w0" },
            { "x": 2, "verifying_key_b64": "vk1", "wrapped_share_b64": "w1" },
            { "x": 3, "verifying_key_b64": "vk2", "wrapped_share_b64": "w2" },
        ]);
        let meta = MintMeta {
            title: "T".into(),
            mime: "text/plain".into(),
            ..Default::default()
        };
        let env = build_metadata_envelope(BuildEnvelope {
            kid_hex: "0123456789abcdef0123456789abcdef",
            asset_cid: "bafytest",
            node_set_id_b64: "nsid",
            shares: &shares,
            meta: &meta,
            creator_principal: "principal:test",
        });
        let prot = &env["asset"]["protections"][0];
        assert_eq!(prot["protectionType"], json!(THRESHOLD_PROTECTION_TYPE));
        assert_eq!(prot["scheme"], json!(THRESHOLD_SCHEME));
        assert_eq!(prot["node_set_id_b64"], json!("nsid"));
        assert_eq!(prot["shares"].as_array().unwrap().len(), 3);
        // No Lit anywhere.
        let s = serde_json::to_string(&env).unwrap();
        assert!(!s.to_lowercase().contains("lit"), "envelope must not mention Lit");
        // The envelope carries no raw key material.
        assert!(assert_no_raw_key_material(&env).is_ok());
    }

    #[test]
    fn raw_key_material_anywhere_fails_closed_but_sealed_shares_pass() {
        // Sealed shares + verifying keys are PUBLIC and must pass.
        assert!(assert_no_raw_key_material(&json!({
            "shares": [ { "wrapped_share_b64": "ok", "verifying_key_b64": "ok" } ]
        }))
        .is_ok());
        // A raw secret anywhere is refused.
        assert!(assert_no_raw_key_material(&json!({ "cek": "leak" })).is_err());
        assert!(assert_no_raw_key_material(&json!({ "a": { "raw_cek": "leak" } })).is_err());
        assert!(assert_no_raw_key_material(&json!({ "x": [ { "seed": "leak" } ] })).is_err());
        assert!(assert_no_raw_key_material(&json!({ "PLAINTEXT_B64": "leak" })).is_err());
    }

    #[test]
    fn channel_fee_native_zero_is_value_zero() {
        let zero = format!("0x{}{}", "0".repeat(64), "0".repeat(64));
        assert_eq!(parse_channel_fee_result(&zero).unwrap(), "0x0");
    }

    #[test]
    fn channel_fee_native_nonzero_carries_into_value() {
        // fee = 0x2386f26fc10000 (0.01 ETH), token = 0x0 (native).
        let fee = format!("{:0>64}", "2386f26fc10000");
        let raw = format!("0x{}{}", fee, "0".repeat(64));
        assert_eq!(parse_channel_fee_result(&raw).unwrap(), "0x2386f26fc10000");
    }

    #[test]
    fn channel_fee_with_erc20_token_fails_closed() {
        // Non-zero token word -> ERC-20 fee we can't cover via msg.value.
        let fee = format!("{:0>64}", "64");
        let token = format!("{:0>64}", "fde4c96c8593536e31f229ea8f37b2ada2699bb2");
        let raw = format!("0x{fee}{token}");
        let err = parse_channel_fee_result(&raw).unwrap_err();
        assert!(err.contains("ERC-20"), "got: {err}");
    }

    #[test]
    fn channel_fee_short_result_fails_closed() {
        assert!(parse_channel_fee_result("0x1234").is_err());
    }

    #[test]
    fn filenames_are_sanitized() {
        assert_eq!(sanitize_filename("my file!.txt"), "my_file_.txt");
        assert_eq!(sanitize_filename(""), "asset.bin");
        assert_eq!(sanitize_filename("../../etc/passwd"), ".._.._etc_passwd");
    }

    #[test]
    fn media_envelope_is_dash_with_single_default_kid_and_dkms_escrow() {
        let shares = json!([
            { "x": 1, "verifying_key_b64": "vk0", "wrapped_share_b64": "w0" },
            { "x": 2, "verifying_key_b64": "vk1", "wrapped_share_b64": "w1" },
            { "x": 3, "verifying_key_b64": "vk2", "wrapped_share_b64": "w2" },
        ]);
        let tracks = vec![
            json!({ "kind": "video", "track_id": 1, "codec": "avc1.64000a", "bandwidth": 29100, "width": 160, "height": 90 }),
            json!({ "kind": "audio", "track_id": 2, "codec": "mp4a.40.2", "bandwidth": 104938, "width": null, "height": null }),
        ];
        let meta = MintMeta { title: "Vid".into(), ..Default::default() };
        let env = build_media_envelope(MediaEnvelope {
            kid_hex: "0123456789abcdef0123456789abcdef",
            dir_cid: "bafydir",
            manifest_path: "manifest.mpd",
            node_set_id_b64: "nsid",
            shares: &shares,
            tracks: &tracks,
            meta: &meta,
            creator_principal: "principal:test",
        });

        // The asset CID is the DASH directory; the media block is DASH with a single default_KID.
        assert_eq!(env["asset"]["assetCid"], json!("bafydir"));
        assert_eq!(env["media"]["mediaType"], json!("dash"));
        assert_eq!(env["media"]["manifestPath"], json!("manifest.mpd"));
        assert_eq!(env["media"]["defaultKID"], json!("0123456789abcdef0123456789abcdef"));
        assert_eq!(env["media"]["tracks"].as_array().unwrap().len(), 2);
        // CEK custody is the SAME dKMS escrow block as the object path (Lit's slot).
        let prot = &env["asset"]["protections"][0];
        assert_eq!(prot["protectionType"], json!(THRESHOLD_PROTECTION_TYPE));
        assert_eq!(prot["node_set_id_b64"], json!("nsid"));
        assert_eq!(prot["shares"].as_array().unwrap().len(), 3);
        // No Lit, and no raw key material.
        let s = serde_json::to_string(&env).unwrap();
        assert!(!s.to_lowercase().contains("lit"), "envelope must not mention Lit");
        assert!(assert_no_raw_key_material(&env).is_ok());
    }
}
