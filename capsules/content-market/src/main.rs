//! ElastOS dDRM Content Market Capsule (Phase C, Day 64).
//!
//! The discovery step: it makes a published mint *findable*. It reconstructs a typed
//! `ContentListingV1` PURELY from a content-mint's calldata — the inverse of
//! `chain-provider::assemble_mint` (Day 62). It holds NO chain RPC, NO IPFS, and NO keys:
//! it mints nothing and reads only the bytes it is handed, so a foreign or malformed call
//! fails closed (no phantom listings).
//!
//! Why a decoder over calldata (and why this is *better* than the PC2 indexer): PC2's
//! `ContentIndexerService` reconstructs a card from FOUR sources — the chain event
//! (`AssetCreated`/`DigitalAssetRegistered`, tokenURI+opType), a `tokenURI()` eth_call,
//! the `metadata.json` (where `content_id` is read from `metadata.kid`,
//! `ContentIndexerService.ts:1106,1117`), and an AuthorityGateway `sellersOf`/`listings`
//! price query. Our Day-62 mint calldata is SELF-DESCRIBING: it already carries the
//! `bytes16 contentId`, the `tokenURI`, the `opType`, AND the sell terms in one verifiable
//! artifact — so a single pure decode yields a complete listing whose `content_id`
//! round-trips to the producer's KID. Human-facing fields (title, poster, mime) still need
//! `metadata.json`; this capsule NAMES `ipfs-provider` + `chain-provider` for that
//! enrichment but performs neither — the runtime's "core injects capabilities" pattern.
//!
//! Fidelity to PC2 (audited in `pc2-node/src/services/ContentIndexerService.ts`):
//!   * the listing's content identity IS the KID (`content_id = metadata.kid`, line 1117) —
//!     here read straight from the `bytes16 contentId` that leads `opRawData`;
//!   * `tokenURI -> metadata CID` via the same `extractCid` rule (line 1140);
//!   * `opType ∈ { FREE=0, BUY_ONCE=1, BUY_AND_RESELL=2 }`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

const LISTING_SCHEMA: &str = "elastos.market.listing/v1";
/// The mint shape we invert (PC2 V3 Channel "Digital Asset").
const MINT_FUNCTION: &str = "mint(string,uint16,bytes,bytes)";

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Status,
    /// Reconstruct a `ContentListingV1` from a content-mint's calldata.
    ReconstructListing {
        request: Box<ReconstructRequestV1>,
    },
    /// Fuse a resolved `metadata.json` onto the calldata-derived identity. The calldata is
    /// authoritative: metadata can only DESCRIBE, never re-identify.
    EnrichListing {
        request: Box<EnrichRequestV1>,
    },
    /// Reconstruct a `ContentListingV1` from a real on-chain mint EVENT log (topics+data),
    /// closing the gap between what we assemble and what the chain actually emits.
    ListingFromEvent {
        request: Box<EventRequestV1>,
    },
    Shutdown,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReconstructRequestV1 {
    /// The mint calldata (`0x` + 4-byte selector + ABI-encoded args) — exactly the `data`
    /// field `chain-provider::assemble_mint` returns / the tx carries on-chain.
    calldata: String,
    /// The Channel contract the mint targets (the tx `to`).
    channel_address: String,
    #[serde(default = "default_chain_id")]
    chain_id: u64,
    /// Optional: if present, the calldata's leading 4-byte selector MUST match, else this
    /// fails closed (it isn't the mint we know how to read).
    #[serde(default)]
    expected_selector: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrichRequestV1 {
    /// The authoritative identity source — the same self-describing mint calldata
    /// `reconstruct_listing` consumes. The contentId derived here, NOT the metadata, fixes
    /// the listing's identity.
    calldata: String,
    channel_address: String,
    #[serde(default = "default_chain_id")]
    chain_id: u64,
    #[serde(default)]
    expected_selector: Option<String>,
    /// The parsed `metadata.json` (handed in by `ipfs-provider`; this capsule fetches
    /// nothing). It may only DESCRIBE the asset; its `kid` MUST match the calldata
    /// contentId or the enrichment is rejected.
    metadata: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EventRequestV1 {
    /// `eth_getLogs` topics; `topics[0]` is the event signature hash. Indexed params follow.
    topics: Vec<String>,
    /// The non-indexed event params, ABI-encoded (`0x` hex) — handed in by `chain-provider`.
    data: String,
    /// The contract that emitted the log (the eventHub/centralStorage or channel).
    address: String,
    #[serde(default = "default_chain_id")]
    chain_id: u64,
}

fn default_chain_id() -> u64 {
    8453 // Base mainnet (PC2 production)
}

// PC2 event topic hashes (keccak256 of the event signature) — ContentIndexerService.ts:59.
const TOPIC_ASSET_CREATED: &str =
    "0xc0a995e4052be044599af577ab2f3382d67bd34df95a76226e7c464e9d4dba46";
const TOPIC_DIGITAL_ASSET_REGISTERED: &str =
    "0x1b24f7763272894608506beba5887c374d345cd231bf52bd03f40bc2d0508d7b";

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
    },
}

impl Response {
    fn ok(data: Value) -> Self {
        Response::Ok { data: Some(data) }
    }

    fn empty_ok() -> Self {
        Response::Ok { data: None }
    }

    fn error(code: &str, message: impl Into<String>) -> Self {
        Response::Error {
            code: code.to_string(),
            message: message.into(),
            details: None,
        }
    }
}

#[derive(Debug, Default)]
struct ContentMarket;

impl ContentMarket {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::ReconstructListing { request } => self.reconstruct_listing(*request),
            Request::EnrichListing { request } => self.enrich_listing(*request),
            Request::ListingFromEvent { request } => self.listing_from_event(*request),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, _config: Value) -> Response {
        Response::ok(json!({
            "provider": "content-market",
            "protocol_version": "1.0",
            "configured": false,
            "supported_operations": ["status", "reconstruct_listing", "enrich_listing", "listing_from_event"],
        }))
    }

    fn status(&self) -> Response {
        Response::ok(json!({
            "provider": "content-market",
            "version": PROVIDER_VERSION,
            "configured": false,
            "supported_operations": ["status", "reconstruct_listing", "enrich_listing", "listing_from_event"],
            "decodes_function": MINT_FUNCTION,
            "decodes_events": ["AssetCreated", "DigitalAssetRegistered"],
            "supported_op_types": ["free", "buy_once", "buy_and_resell"],
            // The listing's identity IS the KID (== bytes16 contentId), no hash/truncation.
            "content_id_rule": "bytes16 == 0x + lowercase(kid_hex[32])",
            // content-market reconstructs a listing but holds no chain/IPFS/keys and mints
            // nothing — discovery is read-only by construction.
            "blocked_authority": [
                "chain_rpc",
                "ipfs",
                "wallet_keys",
                "private_key",
                "mint",
                "broadcast"
            ],
            // Human-facing enrichment (title/poster/mime from metadata.json, live event
            // scanning) is delegated, not performed here.
            "enrich_requires": ["ipfs-provider", "chain-provider"],
        }))
    }

    /// Reconstruct the listing. Pure (no chain/IPFS I/O): decodes the mint calldata back
    /// into a typed `ContentListingV1` whose `content_id` is the `bytes16` KID, closing
    /// KID -> contentId -> listing as ONE identity. Fails closed on any malformed input.
    fn reconstruct_listing(&self, request: ReconstructRequestV1) -> Response {
        if let Err(err) = validate_eth_address(&request.channel_address, "channel_address") {
            return Response::error("invalid_request", err);
        }
        let decoded = match decode_mint_calldata(&request.calldata, request.expected_selector.as_deref()) {
            Ok(d) => d,
            Err(err) => return Response::error("invalid_request", err),
        };
        Response::ok(build_listing(&decoded, &request.channel_address, request.chain_id))
    }

    /// Fuse a resolved `metadata.json` onto the calldata-derived identity. The calldata is
    /// authoritative: we re-derive the contentId from the calldata (NOT from the metadata),
    /// then REQUIRE `metadata.kid == content_id` before attaching any descriptive field. A
    /// mismatched, missing, or malformed kid is rejected — metadata can describe an asset
    /// but can never re-point a listing at a different identity.
    fn enrich_listing(&self, request: EnrichRequestV1) -> Response {
        if let Err(err) = validate_eth_address(&request.channel_address, "channel_address") {
            return Response::error("invalid_request", err);
        }
        let decoded = match decode_mint_calldata(&request.calldata, request.expected_selector.as_deref()) {
            Ok(d) => d,
            Err(err) => return Response::error("invalid_request", err),
        };

        // CARDINAL RULE: metadata.kid (kid || properties.kid) MUST equal the calldata
        // contentId. The calldata identity wins; metadata only describes.
        let meta = &request.metadata;
        let kid_field = meta
            .get("kid")
            .and_then(Value::as_str)
            .or_else(|| meta.get("properties").and_then(|p| p.get("kid")).and_then(Value::as_str));
        let kid = match kid_field {
            Some(k) => k,
            None => return Response::error("invalid_request", "metadata has no kid (kid | properties.kid)"),
        };
        let normalized_kid = match normalize_kid(kid) {
            Ok(id) => id,
            Err(err) => return Response::error("invalid_request", err),
        };
        if normalized_kid != decoded.content_id {
            return Response::error(
                "identity_mismatch",
                format!(
                    "metadata.kid ({normalized_kid}) does not match the calldata contentId ({}) — refusing to re-identify the listing",
                    decoded.content_id
                ),
            );
        }

        let mut listing = build_listing(&decoded, &request.channel_address, request.chain_id);

        // Descriptive fields only (PC2 ContentIndexerService.ts:1102–1128).
        let content_cid = meta
            .get("media")
            .and_then(|m| m.get("uri"))
            .and_then(Value::as_str)
            .and_then(extract_cid);
        let mime_type = meta
            .get("media")
            .and_then(|m| m.get("contentType"))
            .and_then(Value::as_str);
        // image, else media.previewURL (poster fallback).
        let thumbnail = meta
            .get("image")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| meta.get("media").and_then(|m| m.get("previewURL")).and_then(Value::as_str));
        let publisher = meta
            .get("properties")
            .and_then(|p| p.get("publisher"))
            .and_then(Value::as_str);

        listing["name"] = json!(meta.get("name").and_then(Value::as_str));
        listing["description"] = json!(meta.get("description").and_then(Value::as_str));
        listing["image_url"] = json!(thumbnail);
        listing["content_cid"] = json!(content_cid);
        listing["mime_type"] = json!(mime_type);
        listing["asset_type"] = json!(classify_asset_type(mime_type));
        if let Some(p) = publisher {
            listing["creator_address"] = json!(p);
        }
        listing["metadata_status"] = json!("resolved");

        Response::ok(listing)
    }

    /// Reconstruct a listing from a real on-chain mint EVENT log. Proves the chain's own
    /// emission agrees with what we assembled: `DigitalAssetRegistered` carries the
    /// `bytes16 contentId` on-chain, so its listing has the SAME identity as the calldata
    /// path; `AssetCreated` carries no contentId, so identity is DEFERRED
    /// (`metadata_status:"needs_kid"`) to the `enrich_listing` kid-match rather than guessed.
    /// Pure: the log bytes are handed in by `chain-provider`; this capsule fetches nothing.
    fn listing_from_event(&self, request: EventRequestV1) -> Response {
        if let Err(err) = validate_eth_address(&request.address, "address") {
            return Response::error("invalid_request", err);
        }
        let topic0 = match request.topics.first() {
            Some(t) => t.to_lowercase(),
            None => return Response::error("invalid_request", "log has no topics"),
        };

        match topic0.as_str() {
            t if t == TOPIC_DIGITAL_ASSET_REGISTERED => from_digital_asset_registered(&request),
            t if t == TOPIC_ASSET_CREATED => from_asset_created(&request),
            other => Response::error(
                "invalid_request",
                format!("unrecognized event topic {other} (not a known content mint)"),
            ),
        }
    }
}

/// `DigitalAssetRegistered(address indexed channel, uint256 indexed tokenId,
///   address creator, string tokenURI, uint16 opType, bytes16 contentId)`.
/// topics[1] = channel; data = abi.encode(creator, tokenURI, opType, contentId).
fn from_digital_asset_registered(request: &EventRequestV1) -> Response {
        let channel = match request.topics.get(1).map(|t| unpad_topic_address(t)) {
            Some(Ok(addr)) => addr,
            _ => return Response::error("invalid_request", "DigitalAssetRegistered: missing/invalid channel topic"),
        };
        let body = match hex_to_bytes(&request.data) {
            Ok(b) => b,
            Err(err) => return Response::error("invalid_request", err),
        };
        if body.len() < 4 * 32 {
            return Response::error("invalid_request", "DigitalAssetRegistered data truncated (expected 4 head words)");
        }
        let offset_uri = match word_to_usize(word(&body, 1)) {
            Ok(o) => o,
            Err(err) => return Response::error("invalid_request", err),
        };
        let op_code = match word_to_u16(word(&body, 2)) {
            Ok(c) => c,
            Err(err) => return Response::error("invalid_request", err),
        };
        let op_tag = match op_type_tag(op_code) {
            Ok(t) => t,
            Err(err) => return Response::error("invalid_request", err),
        };
        // bytes16 contentId is the first 16 bytes of the 4th head word (left-aligned).
        let content_id = format!("0x{}", hex(&word(&body, 3)[0..16]));
        let token_uri = match decode_abi_string(&body, offset_uri) {
            Ok(u) => u,
            Err(err) => return Response::error("invalid_request", err),
        };

        Response::ok(event_listing(
            Some(&content_id),
            &channel,
            request.chain_id,
            &token_uri,
            op_tag,
            op_code,
            "DigitalAssetRegistered",
            &request.address,
            "unresolved",
        ))
    }

/// `AssetCreated(address indexed _to, address indexed _channel, uint256 _tokenId,
///   string _tokenUri, uint16 _opType, address indexed opContract)`.
/// topics[2] = channel; data = abi.encode(tokenId, tokenURI, opType). No contentId
/// on-chain — identity is deferred to enrichment.
fn from_asset_created(request: &EventRequestV1) -> Response {
        let channel = match request.topics.get(2).map(|t| unpad_topic_address(t)) {
            Some(Ok(addr)) => addr,
            _ => return Response::error("invalid_request", "AssetCreated: missing/invalid channel topic"),
        };
        let body = match hex_to_bytes(&request.data) {
            Ok(b) => b,
            Err(err) => return Response::error("invalid_request", err),
        };
        if body.len() < 3 * 32 {
            return Response::error("invalid_request", "AssetCreated data truncated (expected 3 head words)");
        }
        let offset_uri = match word_to_usize(word(&body, 1)) {
            Ok(o) => o,
            Err(err) => return Response::error("invalid_request", err),
        };
        let op_code = match word_to_u16(word(&body, 2)) {
            Ok(c) => c,
            Err(err) => return Response::error("invalid_request", err),
        };
        let op_tag = match op_type_tag(op_code) {
            Ok(t) => t,
            Err(err) => return Response::error("invalid_request", err),
        };
        let token_uri = match decode_abi_string(&body, offset_uri) {
            Ok(u) => u,
            Err(err) => return Response::error("invalid_request", err),
        };

        Response::ok(event_listing(
            None, // no contentId on-chain — defer to enrich_listing's kid-match
            &channel,
            request.chain_id,
            &token_uri,
            op_tag,
            op_code,
            "AssetCreated",
            &request.address,
            "needs_kid",
        ))
}

/// Build a listing reconstructed from a chain event. `content_id` is `None` for events that
/// carry no on-chain contentId (AssetCreated), which sets `metadata_status:"needs_kid"`.
#[allow(clippy::too_many_arguments)]
fn event_listing(
    content_id: Option<&str>,
    channel: &str,
    chain_id: u64,
    token_uri: &str,
    op_tag: &str,
    op_code: u16,
    event: &str,
    event_source: &str,
    metadata_status: &str,
) -> Value {
    json!({
        "schema": LISTING_SCHEMA,
        "content_id": content_id,
        "channel_address": channel,
        "chain_id": chain_id,
        "token_uri": token_uri,
        "metadata_cid": extract_cid(token_uri),
        "op_type": op_tag,
        "op_type_code": op_code,
        // Provenance: reconstructed from the chain's own emitted log (not the calldata, not
        // a trusted index). For DigitalAssetRegistered this is a complete identity; for
        // AssetCreated the identity is deferred to enrichment.
        "source": "chain_event",
        "event": event,
        "event_source": event_source,
        "metadata_status": metadata_status,
        "enrich_requires": ["ipfs-provider", "chain-provider"],
    })
}

/// A 32-byte indexed topic -> a `0x` 20-byte address (right-aligned, last 20 bytes).
fn unpad_topic_address(topic: &str) -> Result<String, String> {
    let clean = topic.strip_prefix("0x").unwrap_or(topic);
    if clean.len() != 64 || !clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("topic is not a 32-byte word".to_string());
    }
    Ok(format!("0x{}", &clean[24..].to_lowercase()))
}

/// Build the base (calldata-only) listing. `metadata_status` is `unresolved`; the
/// identity/sell fields are complete and verifiable from the calldata alone.
fn build_listing(decoded: &DecodedMint, channel: &str, chain_id: u64) -> Value {
    let mut listing = json!({
        "schema": LISTING_SCHEMA,
        // The producer's KID, carried on-chain as the bytes16 contentId.
        "content_id": decoded.content_id,
        "channel_address": channel,
        "chain_id": chain_id,
        "token_uri": decoded.token_uri,
        "metadata_cid": extract_cid(&decoded.token_uri),
        "op_type": decoded.op_type_tag,
        "op_type_code": decoded.op_type_code,
        // Provenance: reconstructed from the mint calldata itself, not a trusted index —
        // anyone holding the calldata can verify it.
        "source": "mint_calldata",
        "selector": decoded.selector,
        // The crypto/identity fields are complete; the human-facing fields are not (yet).
        "metadata_status": "unresolved",
        "enrich_requires": ["ipfs-provider", "chain-provider"],
    });
    if let Some(sell) = &decoded.sell {
        listing["copies"] = json!(sell.copies);
        listing["price_wei"] = json!(sell.price_wei);
        listing["pay_token"] = json!(sell.pay_token);
    }
    listing
}

/// A metadata `kid` -> the `0x` + 32-lowercase-hex contentId form. Accepts an optional
/// `0x` prefix; rejects anything that is not exactly 16 bytes (32 hex chars) — the same
/// bytes16 rule the producer/publish/chain path enforces (no hash, no truncation).
fn normalize_kid(kid: &str) -> Result<String, String> {
    let clean = kid.strip_prefix("0x").unwrap_or(kid);
    if clean.len() != 32 || !clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("metadata.kid must be 32 hex chars (the 16-byte contentId)".to_string());
    }
    Ok(format!("0x{}", clean.to_lowercase()))
}

/// MIME -> asset class. Mirrors PC2 `classifyAssetType` (ContentIndexerService.ts:114).
fn classify_asset_type(mime: Option<&str>) -> &'static str {
    let m = match mime {
        Some(m) if !m.is_empty() => m,
        _ => return "unknown",
    };
    if m.starts_with("image/") {
        "image"
    } else if m.starts_with("video/") {
        "video"
    } else if m.starts_with("audio/") {
        "audio"
    } else if m.starts_with("text/") {
        "text"
    } else if m == "application/pdf" {
        "document"
    } else if [
        "application/javascript",
        "application/json",
        "application/xml",
        "application/x-yaml",
        "application/toml",
        "application/x-sh",
    ]
    .contains(&m)
    {
        "code"
    } else if m.contains("model") || m.contains("gguf") || m.contains("safetensors") || m.contains("onnx") {
        "ai-model"
    } else if m.contains("font") {
        "font"
    } else if m.contains("gltf") || m.contains("fbx") || m.contains("obj") {
        "3d"
    } else if m.contains("csv") || m.contains("parquet") || m.contains("jsonl") {
        "dataset"
    } else {
        "other"
    }
}

struct SellTerms {
    copies: String,
    price_wei: String,
    pay_token: String,
}

struct DecodedMint {
    selector: String,
    token_uri: String,
    op_type_code: u16,
    op_type_tag: &'static str,
    content_id: String,
    sell: Option<SellTerms>,
}

/// Decode `mint(string _uri, uint16 opType, bytes opRawData, bytes sellRawData)` calldata
/// back into its identity-bearing fields. The inverse of Day-62's `encode_mint_calldata`.
/// Reads ONLY the leading `bytes16 contentId` of `opRawData` (which leads it in both the
/// FREE and PAID encodings) and the `(copies, price, payToken)` of `sellRawData` — the
/// payee/royalty arrays are not needed to identify a listing. Fails closed on any shape
/// that isn't a well-formed mint.
fn decode_mint_calldata(calldata: &str, expected_selector: Option<&str>) -> Result<DecodedMint, String> {
    let bytes = hex_to_bytes(calldata)?;
    if bytes.len() < 4 {
        return Err("calldata too short for a 4-byte selector".to_string());
    }
    let selector = format!("0x{}", hex(&bytes[0..4]));
    if let Some(expected) = expected_selector {
        if !selector.eq_ignore_ascii_case(expected) {
            return Err(format!(
                "selector {selector} does not match expected {expected} — not a known mint"
            ));
        }
    }

    let body = &bytes[4..];
    // Head: 4 words — offset(_uri), opType, offset(opRawData), offset(sellRawData).
    if body.len() < 4 * 32 {
        return Err("calldata head truncated (expected 4 ABI words)".to_string());
    }
    let offset_uri = word_to_usize(word(body, 0))?;
    let op_type_code = word_to_u16(word(body, 1))?;
    let offset_op = word_to_usize(word(body, 2))?;
    let offset_sell = word_to_usize(word(body, 3))?;

    let token_uri = decode_abi_string(body, offset_uri)?;

    // opRawData: a dynamic `bytes`. Its first 16 bytes are the left-aligned bytes16
    // contentId (FREE: abi.encode(bytes16); PAID: the leading static head word).
    let op_raw = decode_abi_bytes(body, offset_op)?;
    if op_raw.len() < 32 {
        return Err("opRawData too short to carry a bytes16 contentId".to_string());
    }
    let content_id = format!("0x{}", hex(&op_raw[0..16]));

    // sellRawData: empty for FREE, else abi.encode(uint256 copies, uint256 price,
    // address payToken). Enforce the op_type/sell consistency PC2 relies on.
    let sell_raw = decode_abi_bytes(body, offset_sell)?;
    let op_type_tag = op_type_tag(op_type_code)?;
    let sell = match op_type_code {
        0 => {
            if !sell_raw.is_empty() {
                return Err("a FREE mint must carry empty sellRawData".to_string());
            }
            None
        }
        _ => {
            if sell_raw.len() < 96 {
                return Err("a PAID mint must carry (copies, price, payToken)".to_string());
            }
            let copies = be_to_decimal(&sell_raw[0..32]);
            let price_wei = be_to_decimal(&sell_raw[32..64]);
            let pay_token = format!("0x{}", hex(&sell_raw[64 + 12..96]));
            Some(SellTerms { copies, price_wei, pay_token })
        }
    };

    Ok(DecodedMint {
        selector,
        token_uri,
        op_type_code,
        op_type_tag,
        content_id,
        sell,
    })
}

/// opType code -> tag. `mint`/event share this mapping; an unknown code fails closed.
fn op_type_tag(code: u16) -> Result<&'static str, String> {
    match code {
        0 => Ok("free"),
        1 => Ok("buy_once"),
        2 => Ok("buy_and_resell"),
        other => Err(format!("unknown opType {other} (expected 0, 1, or 2)")),
    }
}

fn word(body: &[u8], index: usize) -> &[u8] {
    &body[index * 32..index * 32 + 32]
}

/// A 32-byte word -> usize (offsets/lengths). The high 24 bytes MUST be zero; anything
/// larger than a `usize` is a malformed/hostile offset and fails closed.
fn word_to_usize(w: &[u8]) -> Result<usize, String> {
    if w[..24].iter().any(|&b| b != 0) {
        return Err("ABI offset/length exceeds addressable range".to_string());
    }
    let mut v = 0usize;
    for &b in &w[24..32] {
        v = v
            .checked_mul(256)
            .and_then(|x| x.checked_add(b as usize))
            .ok_or("ABI offset/length overflow")?;
    }
    Ok(v)
}

fn word_to_u16(w: &[u8]) -> Result<u16, String> {
    if w[..30].iter().any(|&b| b != 0) {
        return Err("opType word is not a clean uint16".to_string());
    }
    Ok(((w[30] as u16) << 8) | w[31] as u16)
}

fn decode_abi_string(body: &[u8], offset: usize) -> Result<String, String> {
    let raw = decode_abi_bytes(body, offset)?;
    String::from_utf8(raw).map_err(|_| "tokenURI is not valid UTF-8".to_string())
}

/// Read a dynamic `bytes`/`string` at `offset`: a length word followed by `length` bytes.
fn decode_abi_bytes(body: &[u8], offset: usize) -> Result<Vec<u8>, String> {
    let len_end = offset
        .checked_add(32)
        .ok_or("ABI length offset overflow")?;
    if len_end > body.len() {
        return Err("ABI length word out of bounds".to_string());
    }
    let len = word_to_usize(&body[offset..len_end])?;
    let data_end = len_end.checked_add(len).ok_or("ABI data overflow")?;
    if data_end > body.len() {
        return Err("ABI data out of bounds".to_string());
    }
    Ok(body[len_end..data_end].to_vec())
}

/// Big-endian bytes -> base-10 decimal string (handles full uint256 without precision loss).
fn be_to_decimal(bytes: &[u8]) -> String {
    let mut digits: Vec<u8> = vec![0]; // little-endian decimal digits
    for &b in bytes {
        let mut carry = b as u32;
        for d in digits.iter_mut() {
            let v = (*d as u32) * 256 + carry;
            *d = (v % 10) as u8;
            carry = v / 10;
        }
        while carry > 0 {
            digits.push((carry % 10) as u8);
            carry /= 10;
        }
    }
    let s: String = digits.iter().rev().map(|d| (b'0' + d) as char).collect();
    let trimmed = s.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `tokenURI` -> leading IPFS CID. Mirrors PC2 `extractCid` (ContentIndexerService.ts:1140):
/// `ipfs://CID/...`, `.../ipfs/CID/...`, or a bare `Qm…`/`bafy…` path.
fn extract_cid(uri: &str) -> Option<String> {
    if let Some(rest) = uri.strip_prefix("ipfs://") {
        return rest.split('/').next().filter(|s| !s.is_empty()).map(str::to_string);
    }
    if let Some(idx) = uri.find("/ipfs/") {
        return uri[idx + 6..].split('/').next().filter(|s| !s.is_empty()).map(str::to_string);
    }
    if uri.starts_with("Qm") || uri.starts_with("bafy") {
        return uri.split('/').next().filter(|s| !s.is_empty()).map(str::to_string);
    }
    None
}

fn hex_to_bytes(s: &str) -> Result<Vec<u8>, String> {
    let clean = s.strip_prefix("0x").unwrap_or(s);
    if clean.len() % 2 != 0 {
        return Err("calldata hex has an odd length".to_string());
    }
    if !clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("calldata contains non-hex characters".to_string());
    }
    (0..clean.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn validate_eth_address(value: &str, field: &str) -> Result<(), String> {
    let hex = value.strip_prefix("0x").unwrap_or("");
    if value.starts_with("0x") && hex.len() == 40 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(format!("{field} must be a 0x-prefixed 20-byte address"))
    }
}

fn main() {
    eprintln!(
        "content-market: starting v{} (mint -> listing reconstruction)",
        PROVIDER_VERSION
    );

    let mut provider = ContentMarket;
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("content-market read error: {err}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(err) => {
                let response = Response::error("invalid_request", err.to_string());
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

    eprintln!("content-market exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    const KID: &str = "38691296765e76a331f5d5630bddf9f5";
    const CHANNEL: &str = "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D";
    const SELECTOR: &str = "0xaabbccdd";
    const META_CID: &str = "QmMetaFolderCidV0";

    fn ok_data(response: Response) -> Value {
        match response {
            Response::Ok { data: Some(data) } => data,
            other => panic!("expected ok data, got {other:?}"),
        }
    }

    fn error_code(response: Response) -> String {
        match response {
            Response::Error { code, .. } => code,
            other => panic!("expected error, got {other:?}"),
        }
    }

    // ── Minimal ABI tuple encoder (mirrors chain-provider's layout) so tests can build
    // mint calldata to decode. The CROSS-BINARY smoke proves byte-compat with the real
    // chain-provider encoder; here we just need a well-formed inverse to characterize.

    fn pad_left_u64(n: u64) -> Vec<u8> {
        let mut w = vec![0u8; 32];
        w[24..32].copy_from_slice(&n.to_be_bytes());
        w
    }

    fn pad_left_u16(n: u16) -> Vec<u8> {
        let mut w = vec![0u8; 32];
        w[30..32].copy_from_slice(&n.to_be_bytes());
        w
    }

    fn bytes16_word(kid_hex: &str) -> Vec<u8> {
        let raw = hex_to_bytes(kid_hex).unwrap();
        let mut w = vec![0u8; 32];
        w[0..16].copy_from_slice(&raw[0..16]); // left-aligned bytes16
        w
    }

    fn address_word(addr: &str) -> Vec<u8> {
        let raw = hex_to_bytes(addr).unwrap();
        let mut w = vec![0u8; 32];
        w[12..32].copy_from_slice(&raw);
        w
    }

    fn dyn_bytes(payload: &[u8]) -> Vec<u8> {
        let mut out = pad_left_u64(payload.len() as u64);
        out.extend_from_slice(payload);
        let rem = payload.len() % 32;
        if rem != 0 {
            out.extend(std::iter::repeat(0u8).take(32 - rem));
        }
        out
    }

    /// Build mint(string,uint16,bytes,bytes) calldata. Dynamic args (string, opRaw, sell)
    /// go in the tail; opType is the static head word.
    fn build_mint(selector: &str, uri: &str, op_code: u16, op_raw: &[u8], sell_raw: &[u8]) -> String {
        let sel = hex_to_bytes(selector).unwrap();
        let uri_enc = dyn_bytes(uri.as_bytes());
        let op_enc = dyn_bytes(op_raw);
        let sell_enc = dyn_bytes(sell_raw);

        let head = 4 * 32usize;
        let off_uri = head;
        let off_op = off_uri + uri_enc.len();
        let off_sell = off_op + op_enc.len();

        let mut body = Vec::new();
        body.extend(pad_left_u64(off_uri as u64));
        body.extend(pad_left_u16(op_code));
        body.extend(pad_left_u64(off_op as u64));
        body.extend(pad_left_u64(off_sell as u64));
        body.extend(uri_enc);
        body.extend(op_enc);
        body.extend(sell_enc);

        let mut all = sel;
        all.extend(body);
        format!("0x{}", hex(&all))
    }

    fn free_calldata() -> String {
        build_mint(SELECTOR, &format!("{META_CID}/metadata.json"), 0, &bytes16_word(KID), &[])
    }

    fn paid_calldata(op_code: u16) -> String {
        let mut sell = pad_left_u64(100); // copies
        sell.extend(be_decimal_to_word("1000000000000000000")); // price 1e18
        sell.extend(address_word("0x0000000000000000000000000000000000000000")); // native
        build_mint(SELECTOR, &format!("{META_CID}/metadata.json"), op_code, &bytes16_word(KID), &sell)
    }

    fn be_decimal_to_word(dec: &str) -> Vec<u8> {
        // Small helper: decimal string -> 32-byte big-endian word (test prices fit u128).
        let n: u128 = dec.parse().unwrap();
        let mut w = vec![0u8; 32];
        w[16..32].copy_from_slice(&n.to_be_bytes());
        w
    }

    fn reconstruct(calldata: &str, expected_selector: Option<&str>) -> Response {
        let req = ReconstructRequestV1 {
            calldata: calldata.to_string(),
            channel_address: CHANNEL.to_string(),
            chain_id: 8453,
            expected_selector: expected_selector.map(str::to_string),
        };
        ContentMarket.reconstruct_listing(req)
    }

    #[test]
    fn free_calldata_reconstructs_listing_with_kid_content_id() {
        let data = ok_data(reconstruct(&free_calldata(), Some(SELECTOR)));
        // The listing's identity IS the producer's KID, 16 bytes.
        assert_eq!(data["content_id"], format!("0x{KID}"));
        assert_eq!(data["op_type"], "free");
        assert_eq!(data["token_uri"], format!("{META_CID}/metadata.json"));
        assert_eq!(data["metadata_cid"], META_CID);
        assert!(data["price_wei"].is_null());
        assert_eq!(data["metadata_status"], "unresolved");
    }

    #[test]
    fn paid_calldata_reconstructs_sell_terms() {
        let data = ok_data(reconstruct(&paid_calldata(1), Some(SELECTOR)));
        assert_eq!(data["content_id"], format!("0x{KID}"));
        assert_eq!(data["op_type"], "buy_once");
        assert_eq!(data["copies"], "100");
        assert_eq!(data["price_wei"], "1000000000000000000");
        assert_eq!(data["pay_token"], "0x0000000000000000000000000000000000000000");
    }

    #[test]
    fn buy_and_resell_op_type_is_recognized() {
        let data = ok_data(reconstruct(&paid_calldata(2), Some(SELECTOR)));
        assert_eq!(data["op_type"], "buy_and_resell");
        assert_eq!(data["op_type_code"], 2);
    }

    #[test]
    fn content_id_round_trips_to_the_producer_kid() {
        // The whole point: KID -> contentId (publish) -> calldata (chain) -> listing.
        let data = ok_data(reconstruct(&paid_calldata(1), None));
        let id = data["content_id"].as_str().unwrap();
        assert_eq!(id.strip_prefix("0x").unwrap(), KID);
    }

    #[test]
    fn foreign_selector_fails_closed() {
        assert_eq!(error_code(reconstruct(&free_calldata(), Some("0xdeadbeef"))), "invalid_request");
    }

    #[test]
    fn truncated_calldata_fails_closed() {
        assert_eq!(error_code(reconstruct("0xaabbccdd0011", Some(SELECTOR))), "invalid_request");
    }

    #[test]
    fn free_with_sale_terms_fails_closed() {
        // op_type FREE but a non-empty sellRawData is a malformed/foreign call.
        let mut sell = pad_left_u64(1);
        sell.extend(be_decimal_to_word("5"));
        sell.extend(address_word("0x0000000000000000000000000000000000000000"));
        let bad = build_mint(SELECTOR, &format!("{META_CID}/metadata.json"), 0, &bytes16_word(KID), &sell);
        assert_eq!(error_code(reconstruct(&bad, Some(SELECTOR))), "invalid_request");
    }

    #[test]
    fn paid_without_sale_terms_fails_closed() {
        let bad = build_mint(SELECTOR, &format!("{META_CID}/metadata.json"), 1, &bytes16_word(KID), &[]);
        assert_eq!(error_code(reconstruct(&bad, Some(SELECTOR))), "invalid_request");
    }

    #[test]
    fn short_op_raw_without_bytes16_fails_closed() {
        let bad = build_mint(SELECTOR, &format!("{META_CID}/metadata.json"), 0, &[1, 2, 3, 4], &[]);
        assert_eq!(error_code(reconstruct(&bad, Some(SELECTOR))), "invalid_request");
    }

    #[test]
    fn unknown_op_type_fails_closed() {
        let bad = build_mint(SELECTOR, &format!("{META_CID}/metadata.json"), 7, &bytes16_word(KID), &[]);
        assert_eq!(error_code(reconstruct(&bad, Some(SELECTOR))), "invalid_request");
    }

    #[test]
    fn bad_channel_address_fails_closed() {
        let req = ReconstructRequestV1 {
            calldata: free_calldata(),
            channel_address: "not-an-address".to_string(),
            chain_id: 8453,
            expected_selector: Some(SELECTOR.to_string()),
        };
        assert_eq!(error_code(ContentMarket.reconstruct_listing(req)), "invalid_request");
    }

    #[test]
    fn extract_cid_matches_pc2_rules() {
        assert_eq!(extract_cid("QmAbc/metadata.json").as_deref(), Some("QmAbc"));
        assert_eq!(extract_cid("ipfs://bafyXYZ/metadata.json").as_deref(), Some("bafyXYZ"));
        assert_eq!(extract_cid("https://gw/ipfs/QmAbc/metadata.json").as_deref(), Some("QmAbc"));
        assert_eq!(extract_cid("https://example.com/x"), None);
    }

    // ── enrich_listing ────────────────────────────────────────────────────────────

    fn metadata(kid: &str) -> Value {
        json!({
            "schema": "elacity-asset-envelope-v1",
            "name": "My Film",
            "description": "A short film.",
            "image": "ipfs://QmPoster/poster.png",
            "kid": kid,
            "media": {
                "uri": "ipfs://QmContent/video.mp4",
                "contentType": "video/mp4",
                "previewURL": "ipfs://QmPreview/preview.jpg"
            },
            "properties": { "publisher": CREATOR_PUB }
        })
    }

    const CREATOR_PUB: &str = "0x2222222222222222222222222222222222222222";

    fn enrich(calldata: &str, meta: Value) -> Response {
        let req = EnrichRequestV1 {
            calldata: calldata.to_string(),
            channel_address: CHANNEL.to_string(),
            chain_id: 8453,
            expected_selector: Some(SELECTOR.to_string()),
            metadata: meta,
        };
        ContentMarket.enrich_listing(req)
    }

    #[test]
    fn enrich_fuses_metadata_onto_the_calldata_identity() {
        let data = ok_data(enrich(&paid_calldata(1), metadata(KID)));
        // Identity is unchanged (calldata is authoritative) and now resolved.
        assert_eq!(data["content_id"], format!("0x{KID}"));
        assert_eq!(data["metadata_status"], "resolved");
        // Descriptive fields fused from metadata.json.
        assert_eq!(data["name"], "My Film");
        assert_eq!(data["description"], "A short film.");
        assert_eq!(data["image_url"], "ipfs://QmPoster/poster.png");
        assert_eq!(data["content_cid"], "QmContent");
        assert_eq!(data["mime_type"], "video/mp4");
        assert_eq!(data["asset_type"], "video");
        assert_eq!(data["creator_address"], CREATOR_PUB);
        // Sell terms still present from the calldata.
        assert_eq!(data["price_wei"], "1000000000000000000");
    }

    #[test]
    fn enrich_accepts_0x_prefixed_and_uppercase_kid() {
        let data = ok_data(enrich(&free_calldata(), metadata(&format!("0x{}", KID.to_uppercase()))));
        assert_eq!(data["content_id"], format!("0x{KID}"));
        assert_eq!(data["metadata_status"], "resolved");
    }

    #[test]
    fn enrich_reads_kid_from_properties_when_top_level_absent() {
        let mut meta = metadata(KID);
        meta.as_object_mut().unwrap().remove("kid");
        meta["properties"]["kid"] = json!(KID);
        let data = ok_data(enrich(&free_calldata(), meta));
        assert_eq!(data["content_id"], format!("0x{KID}"));
    }

    #[test]
    fn enrich_falls_back_to_preview_url_when_image_empty() {
        let mut meta = metadata(KID);
        meta["image"] = json!("");
        let data = ok_data(enrich(&free_calldata(), meta));
        assert_eq!(data["image_url"], "ipfs://QmPreview/preview.jpg");
    }

    #[test]
    fn enrich_rejects_a_kid_that_does_not_match_the_calldata() {
        // The attack: metadata tries to re-point the listing at a different identity.
        let other = "ffffffffffffffffffffffffffffffff";
        match enrich(&paid_calldata(1), metadata(other)) {
            Response::Error { code, .. } => assert_eq!(code, "identity_mismatch"),
            other => panic!("expected identity_mismatch, got {other:?}"),
        }
    }

    #[test]
    fn enrich_rejects_missing_kid() {
        let mut meta = metadata(KID);
        meta.as_object_mut().unwrap().remove("kid");
        assert_eq!(error_code(enrich(&free_calldata(), meta)), "invalid_request");
    }

    #[test]
    fn enrich_rejects_malformed_kid() {
        assert_eq!(error_code(enrich(&free_calldata(), metadata("not-a-kid"))), "invalid_request");
    }

    #[test]
    fn enrich_fails_closed_on_foreign_calldata() {
        // The identity source must be a valid mint even when enriching.
        assert_eq!(error_code(enrich("0xaabbccdd0011", metadata(KID))), "invalid_request");
    }

    #[test]
    fn classify_asset_type_matches_pc2() {
        assert_eq!(classify_asset_type(Some("image/png")), "image");
        assert_eq!(classify_asset_type(Some("application/pdf")), "document");
        assert_eq!(classify_asset_type(Some("application/json")), "code");
        assert_eq!(classify_asset_type(Some("model/gguf")), "ai-model");
        assert_eq!(classify_asset_type(Some("text/csv")), "text"); // text/ wins first
        assert_eq!(classify_asset_type(Some("application/parquet")), "dataset");
        assert_eq!(classify_asset_type(None), "unknown");
        assert_eq!(classify_asset_type(Some("application/zip")), "other");
    }

    // ── listing_from_event ────────────────────────────────────────────────────────

    fn pad_topic_address(addr: &str) -> String {
        format!("0x{}", hex(&address_word(addr)))
    }

    fn dyn_string(s: &str) -> Vec<u8> {
        dyn_bytes(s.as_bytes())
    }

    /// DigitalAssetRegistered data = abi.encode(address creator, string tokenURI,
    /// uint16 opType, bytes16 contentId). 4 head words; string in the tail (offset 128).
    fn dar_data(creator: &str, uri: &str, op_code: u16, kid: &str) -> String {
        let mut body = address_word(creator);
        body.extend(pad_left_u64(4 * 32)); // offset to string
        body.extend(pad_left_u16(op_code));
        body.extend(bytes16_word(kid));
        body.extend(dyn_string(uri));
        format!("0x{}", hex(&body))
    }

    /// AssetCreated data = abi.encode(uint256 tokenId, string tokenURI, uint16 opType).
    fn ac_data(token_id: u64, uri: &str, op_code: u16) -> String {
        let mut body = pad_left_u64(token_id);
        body.extend(pad_left_u64(3 * 32)); // offset to string
        body.extend(pad_left_u16(op_code));
        body.extend(dyn_string(uri));
        format!("0x{}", hex(&body))
    }

    fn from_event(topics: Vec<String>, data: &str, address: &str) -> Response {
        let req = EventRequestV1 {
            topics,
            data: data.to_string(),
            address: address.to_string(),
            chain_id: 8453,
        };
        ContentMarket.listing_from_event(req)
    }

    const EVENT_HUB: &str = "0x3333333333333333333333333333333333333333";

    #[test]
    fn digital_asset_registered_event_matches_calldata_identity() {
        let uri = format!("{META_CID}/metadata.json");
        let topics = vec![
            TOPIC_DIGITAL_ASSET_REGISTERED.to_string(),
            pad_topic_address(CHANNEL),
            format!("0x{}", hex(&pad_left_u64(1))), // tokenId (indexed)
        ];
        let data = dar_data(CREATOR_PUB, &uri, 1, KID);
        let listing = ok_data(from_event(topics, &data, EVENT_HUB));

        // Same identity the calldata path produced.
        assert_eq!(listing["content_id"], format!("0x{KID}"));
        assert_eq!(listing["token_uri"], uri);
        assert_eq!(listing["op_type"], "buy_once");
        assert_eq!(listing["metadata_cid"], META_CID);
        assert_eq!(listing["source"], "chain_event");
        assert_eq!(listing["event"], "DigitalAssetRegistered");
        assert_eq!(listing["metadata_status"], "unresolved");
        // channel comes from the indexed topic, not the emitting contract.
        assert_eq!(listing["channel_address"], CHANNEL.to_lowercase());

        // Cross-check: the calldata path yields the SAME content_id/token_uri/op_type.
        let from_calldata = ok_data(reconstruct(&paid_calldata(1), Some(SELECTOR)));
        assert_eq!(listing["content_id"], from_calldata["content_id"]);
        assert_eq!(listing["token_uri"], from_calldata["token_uri"]);
        assert_eq!(listing["op_type"], from_calldata["op_type"]);
    }

    #[test]
    fn asset_created_event_defers_identity_with_needs_kid() {
        let uri = format!("{META_CID}/metadata.json");
        let topics = vec![
            TOPIC_ASSET_CREATED.to_string(),
            pad_topic_address(CREATOR_PUB), // _to
            pad_topic_address(CHANNEL),     // _channel
            pad_topic_address(EVENT_HUB),   // opContract
        ];
        let data = ac_data(42, &uri, 2);
        let listing = ok_data(from_event(topics, &data, EVENT_HUB));

        // No contentId on-chain — identity deferred, not guessed.
        assert!(listing["content_id"].is_null());
        assert_eq!(listing["metadata_status"], "needs_kid");
        assert_eq!(listing["op_type"], "buy_and_resell");
        assert_eq!(listing["token_uri"], uri);
        assert_eq!(listing["channel_address"], CHANNEL.to_lowercase());
        assert_eq!(listing["event"], "AssetCreated");
    }

    #[test]
    fn unknown_event_topic_fails_closed() {
        let topics = vec!["0xdeadbeef".to_string(), pad_topic_address(CHANNEL)];
        assert_eq!(error_code(from_event(topics, "0x", EVENT_HUB)), "invalid_request");
    }

    #[test]
    fn event_with_no_topics_fails_closed() {
        assert_eq!(error_code(from_event(vec![], "0x", EVENT_HUB)), "invalid_request");
    }

    #[test]
    fn event_truncated_data_fails_closed() {
        let topics = vec![
            TOPIC_DIGITAL_ASSET_REGISTERED.to_string(),
            pad_topic_address(CHANNEL),
            format!("0x{}", hex(&pad_left_u64(1))),
        ];
        assert_eq!(error_code(from_event(topics, "0x0011", EVENT_HUB)), "invalid_request");
    }

    #[test]
    fn event_bad_emitter_address_fails_closed() {
        let topics = vec![TOPIC_DIGITAL_ASSET_REGISTERED.to_string(), pad_topic_address(CHANNEL)];
        assert_eq!(error_code(from_event(topics, "0x", "not-an-address")), "invalid_request");
    }

    #[test]
    fn event_unknown_op_type_fails_closed() {
        let topics = vec![
            TOPIC_DIGITAL_ASSET_REGISTERED.to_string(),
            pad_topic_address(CHANNEL),
            format!("0x{}", hex(&pad_left_u64(1))),
        ];
        let data = dar_data(CREATOR_PUB, &format!("{META_CID}/metadata.json"), 9, KID);
        assert_eq!(error_code(from_event(topics, &data, EVENT_HUB)), "invalid_request");
    }

    #[test]
    fn status_holds_no_authority_and_names_enrichers() {
        let data = ok_data(ContentMarket.status());
        let blocked = data["blocked_authority"].as_array().unwrap();
        for must in ["chain_rpc", "ipfs", "wallet_keys", "mint", "broadcast"] {
            assert!(blocked.iter().any(|v| v == must), "must block {must}");
        }
        assert_eq!(data["enrich_requires"][0], "ipfs-provider");
        assert_eq!(data["enrich_requires"][1], "chain-provider");
    }
}
