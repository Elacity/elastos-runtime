//! ElastOS dDRM Publish Provider Capsule (Phase C, Day 61).
//!
//! The on-chain producer step: takes a producer-sealed asset's identity (the KID) plus
//! its IPFS metadata folder and ASSEMBLES the content mint — but holds NO chain-RPC and
//! NO wallet key itself. It emits a typed, *unsigned* `UnsignedMintV1` for
//! `chain-provider` to ABI-encode + broadcast and `wallet-provider` to sign, exactly the
//! runtime's "core injects capabilities" pattern. Fail-closed: it can `prepare` a mint
//! but can never put anything on-chain on its own.
//!
//! Fidelity to PC2 (audited in `pc2-node/data/test-apps/elacity-creator/app.js`):
//!   * the on-chain `contentId` IS the KID — `0x` + 32 lowercase hex (16 bytes), no
//!     hash, no truncation (`kidToContentId`, app.js:1568) — the SAME `bytes16 contentId`
//!     the consumer chain reads via `hasAccessByContentId(address, bytes16)`;
//!   * mint is `mint(string _uri, uint16 opType, bytes opRawData, bytes sellRawData)` on
//!     the creator's Channel contract (app.js:4948), with
//!     `_uri = {metadataFolderCid}/metadata.json` (app.js:4946);
//!   * `opType` ∈ { FREE=0, BUY_ONCE=1, BUY_AND_RESELL=2 } (app.js:55);
//!   * `opRawData` leads with the `bytes16 contentId` (app.js:1620).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

const PUBLISH_REQUEST_SCHEMA: &str = "elastos.publish.request/v1";
const PUBLISH_RECEIPT_SCHEMA: &str = "elastos.publish.receipt/v1";
const UNSIGNED_MINT_SCHEMA: &str = "elastos.publish.unsigned_mint/v1";

/// PC2 V3 mint selector shape (Channel "Digital Asset" contract).
const MINT_FUNCTION: &str = "mint(string,uint16,bytes,bytes)";
/// The on-chain fee that funds the mint is read from CENTRAL_STORAGE at broadcast time
/// by the chain capability — publish-provider never reads chain state, so it names the
/// source and leaves the value for `chain-provider` to fill.
const FEE_SOURCE: &str = "central_storage:media_creation_fee";

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Status,
    /// Assemble the unsigned content mint from a sealed asset's identity + metadata.
    PreparePublish {
        request: Box<PublishRequestV1>,
    },
    Shutdown,
}

/// On-chain monetisation mode (PC2 `OP_TYPES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OpType {
    Free,
    BuyOnce,
    BuyAndResell,
}

impl OpType {
    fn code(self) -> u16 {
        match self {
            OpType::Free => 0,
            OpType::BuyOnce => 1,
            OpType::BuyAndResell => 2,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            OpType::Free => "free",
            OpType::BuyOnce => "buy_once",
            OpType::BuyAndResell => "buy_and_resell",
        }
    }

    fn is_paid(self) -> bool {
        !matches!(self, OpType::Free)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishRequestV1 {
    schema: String,
    request_id: String,
    /// The producer-minted KID (32 hex chars / 16 bytes) — becomes the on-chain
    /// `bytes16 contentId`. Same value `encrypt-provider` (Day 58) binds.
    kid_hex: String,
    /// IPFS folder CID (CIDv0) holding `metadata.json` + sidecars; `_uri` is derived as
    /// `{metadata_cid}/metadata.json`.
    metadata_cid: String,
    /// The creator's Channel contract (the mint `to`).
    channel_address: String,
    #[serde(default = "default_chain_id")]
    chain_id: u64,
    op_type: OpType,
    /// Paid listings only: price in the smallest unit (wei-like), as a decimal string.
    #[serde(default)]
    price_wei: Option<String>,
    /// Paid listings only: ERC-20 payment token address (omit for native).
    #[serde(default)]
    currency_address: Option<String>,
    /// Paid listings only: number of access-token copies to mint.
    #[serde(default)]
    copies: Option<u64>,
    /// Paid listings only: the creator's payout address (the ACCESS_TOKEN holder + the
    /// default royalty payee). Required for a paid mint.
    #[serde(default)]
    creator_address: Option<String>,
    /// Paid listings only: royalty splits. Defaults to a single 100−ELACITY_ROYALTY_PERCENT
    /// share to the creator (PC2 app.js:1596).
    #[serde(default)]
    royalties: Option<Vec<RoyaltyPartner>>,
    /// BUY_AND_RESELL only: resale royalty in basis points (PC2 default 900).
    #[serde(default)]
    reseller_cut: Option<u16>,
}

/// A royalty payee (PC2 `royalties[]`): `royalty` is a percent (the on-chain amount is
/// `round(10 * royalty)`, app.js:1608). `identifier` "C" marks the distributor for
/// BUY_AND_RESELL's DISTRIBUTION_RIGHT entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoyaltyPartner {
    address: String,
    royalty: f64,
    #[serde(default)]
    identifier: Option<String>,
}

/// PC2 role types (app.js:56–58).
const ROLE_ACCESS_TOKEN: u64 = 1;
const ROLE_ROYALTY_SHARE: u64 = 2;
const ROLE_DISTRIBUTION_RIGHT: u64 = 3;
/// Protocol cut taken automatically; the creator's default share is the remainder.
const ELACITY_ROYALTY_PERCENT: f64 = 5.0;
/// PC2 default resale royalty (basis points), app.js getResellerCut fallback.
const DEFAULT_RESELLER_CUT: u16 = 900;
/// Native (no ERC-20) payment token.
const ZERO_ADDRESS: &str = "0x0000000000000000000000000000000000000000";

fn default_chain_id() -> u64 {
    8453 // Base mainnet (PC2 production)
}

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
struct PublishProvider;

impl PublishProvider {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::PreparePublish { request } => self.prepare_publish(*request),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, _config: Value) -> Response {
        Response::ok(json!({
            "provider": "publish",
            "protocol_version": "1.0",
            "configured": false,
            "supported_operations": ["status", "prepare_publish"],
        }))
    }

    fn status(&self) -> Response {
        Response::ok(json!({
            "provider": "publish",
            "version": PROVIDER_VERSION,
            "configured": false,
            "supported_operations": ["status", "prepare_publish"],
            "on_chain_function": MINT_FUNCTION,
            "supported_op_types": ["free", "buy_once", "buy_and_resell"],
            // contentId == KID, 0x + 32 lowercase hex (16 bytes); no hash, no truncation.
            "content_id_rule": "bytes16 == 0x + lowercase(kid_hex[32])",
            // publish-provider can ASSEMBLE a mint but never sign or broadcast it.
            "blocked_authority": [
                "chain_rpc",
                "wallet_rpc",
                "wallet_keys",
                "private_key",
                "broadcast"
            ],
            "next_required_providers": ["chain-provider", "wallet-provider"],
        }))
    }

    /// Assemble the unsigned mint. Pure (no chain/wallet I/O): validates the request,
    /// binds `contentId == bytes16 KID`, derives the tokenURI the PC2 way, and emits a
    /// typed `UnsignedMintV1` + a `PublishReceiptV1` whose status is `prepared` (never
    /// `published`) and which names the two providers that must complete the loop.
    fn prepare_publish(&self, request: PublishRequestV1) -> Response {
        if let Err(err) = validate_publish_request(&request) {
            return Response::error("invalid_request", err);
        }

        let content_id = match kid_to_content_id(&request.kid_hex) {
            Ok(id) => id,
            Err(err) => return Response::error("invalid_request", err),
        };
        let token_uri = format!("{}/metadata.json", request.metadata_cid);

        // The structured args the chain capability ABI-encodes. opRawData/sellRawData are
        // left STRUCTURED (not raw calldata, in EXACTLY the shape chain-provider's
        // `assemble_mint` consumes) so the EVM specialist owns the encoding and
        // publish-provider stays capability-clean.
        let (op_raw, sell) = if request.op_type.is_paid() {
            match build_paid_terms(&request, &token_uri) {
                Ok(terms) => terms,
                Err(err) => return Response::error("invalid_request", err),
            }
        } else {
            (Value::Null, Value::Null)
        };

        let unsigned_mint = json!({
            "schema": UNSIGNED_MINT_SCHEMA,
            "chain_id": request.chain_id,
            "to": request.channel_address,
            "function": MINT_FUNCTION,
            "token_uri": token_uri,
            "op_type": request.op_type.tag(),
            "op_type_code": request.op_type.code(),
            // opRawData leads with the bytes16 contentId (PC2 app.js:1620).
            "content_id": content_id,
            // Directly consumable by chain-provider::assemble_mint (paid only).
            "op_raw": op_raw,
            "sell": sell,
            // The mint is payable; the fee comes from chain state, not from us.
            "value_wei": Value::Null,
            "fee_source": FEE_SOURCE,
            "signed": false,
        });

        let receipt = json!({
            "schema": PUBLISH_RECEIPT_SCHEMA,
            "request_id": request.request_id,
            "provider": "publish-provider",
            // The identity that ties producer (encrypt KID) -> chain (contentId) ->
            // consumer (rights/decrypt object) into one loop.
            "content_id": content_id,
            "token_uri": token_uri,
            "op_type": request.op_type.tag(),
            "channel_address": request.channel_address,
            "chain_id": request.chain_id,
            // NEVER "published": this capsule cannot broadcast. The runtime must drive
            // the named providers to finish the on-chain step.
            "status": "prepared",
            "requires": ["chain-provider", "wallet-provider"],
            "unsigned_mint": unsigned_mint,
        });

        Response::ok(receipt)
    }
}

/// Build the PAID `op_raw` + `sell` terms in the EXACT shape `chain-provider::
/// assemble_mint` consumes. Mirrors PC2 `encodeOpRawData` (app.js:1583): addresses lead
/// with the creator as the ACCESS_TOKEN holder, then a ROYALTY_SHARE per payee
/// (`amount = round(10 * royalty)`), plus a DISTRIBUTION_RIGHT for the BUY_AND_RESELL
/// distributor (identifier "C", distinct from the creator). `metadata_uri = ipfs://{cid}`.
fn build_paid_terms(request: &PublishRequestV1, _token_uri: &str) -> Result<(Value, Value), String> {
    let creator = request
        .creator_address
        .as_deref()
        .ok_or("creator_address is required for a paid op_type")?;
    let copies = request.copies.unwrap_or(1);
    let resellable = request.op_type == OpType::BuyAndResell;

    let royalties = request.royalties.clone().unwrap_or_else(|| {
        vec![RoyaltyPartner {
            address: creator.to_string(),
            royalty: 100.0 - ELACITY_ROYALTY_PERCENT,
            identifier: Some("A".to_string()),
        }]
    });

    let mut addresses: Vec<String> = vec![creator.to_string()];
    let mut role_types: Vec<u64> = vec![ROLE_ACCESS_TOKEN];
    let mut amounts: Vec<String> = vec![copies.to_string()];

    for r in &royalties {
        validate_eth_address(&r.address, "royalties[].address")?;
        if !(r.royalty.is_finite() && r.royalty >= 0.0) {
            return Err("royalties[].royalty must be a non-negative number".to_string());
        }
        addresses.push(r.address.clone());
        role_types.push(ROLE_ROYALTY_SHARE);
        amounts.push(((10.0 * r.royalty).round() as u64).to_string());
    }

    if resellable {
        if let Some(dist) = royalties
            .iter()
            .find(|r| r.identifier.as_deref() == Some("C"))
        {
            if !dist.address.eq_ignore_ascii_case(creator) {
                addresses.push(dist.address.clone());
                role_types.push(ROLE_DISTRIBUTION_RIGHT);
                amounts.push("1".to_string());
            }
        }
    }

    let pay_token = request
        .currency_address
        .as_deref()
        .filter(|a| !a.is_empty())
        .unwrap_or(ZERO_ADDRESS)
        .to_string();

    let mut op_raw = json!({
        "metadata_uri": format!("ipfs://{}", request.metadata_cid),
        "addresses": addresses,
        "role_types": role_types,
        "amounts": amounts,
    });
    if resellable {
        op_raw["reseller_cut"] = json!(request.reseller_cut.unwrap_or(DEFAULT_RESELLER_CUT));
    }

    let sell = json!({
        "copies": copies.to_string(),
        "price_wei": request.price_wei,
        "pay_token": pay_token,
    });

    Ok((op_raw, sell))
}

/// KID -> on-chain `bytes16` contentId. Mirrors PC2 `kidToContentId` (app.js:1568) and
/// `encrypt-provider::kid_to_content_id_bytes16` (Day 58): exactly 32 hex chars,
/// `0x`-prefixed, lowercased. No hash, no truncation — the legacy hash-derived contentId
/// was deliberately removed in PC2 and must not be reintroduced.
fn kid_to_content_id(kid_hex: &str) -> Result<String, String> {
    let clean = kid_hex.strip_prefix("0x").unwrap_or(kid_hex);
    if clean.len() != 32 || !clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(
            "kid_hex must be 32 hex chars (the 16-byte bytes16 contentId); no hashing".to_string(),
        );
    }
    Ok(format!("0x{}", clean.to_lowercase()))
}

fn validate_publish_request(request: &PublishRequestV1) -> Result<(), String> {
    if request.schema != PUBLISH_REQUEST_SCHEMA {
        return Err(format!(
            "schema must be {PUBLISH_REQUEST_SCHEMA}, got {}",
            request.schema
        ));
    }
    require_non_empty(&request.request_id, "request_id")?;
    require_non_empty(&request.metadata_cid, "metadata_cid")?;
    validate_eth_address(&request.channel_address, "channel_address")?;

    if request.op_type.is_paid() {
        // A paid listing without a price is fail-closed: we will not mint a sellable
        // asset with no price.
        match request.price_wei.as_deref() {
            Some(p) if !p.is_empty() => validate_decimal(p, "price_wei")?,
            _ => return Err("price_wei is required for a paid op_type".to_string()),
        }
        // The creator's payout address (ACCESS_TOKEN holder + default royalty payee).
        match request.creator_address.as_deref() {
            Some(addr) if !addr.is_empty() => validate_eth_address(addr, "creator_address")?,
            _ => return Err("creator_address is required for a paid op_type".to_string()),
        }
        if let Some(addr) = request.currency_address.as_deref() {
            if !addr.is_empty() {
                validate_eth_address(addr, "currency_address")?;
            }
        }
    } else {
        // FREE must NOT carry sale terms (PC2 encodes only the bytes16 for FREE).
        if request.price_wei.as_deref().is_some_and(|p| !p.is_empty())
            || request.currency_address.as_deref().is_some_and(|a| !a.is_empty())
            || request.copies.is_some()
            || request.creator_address.as_deref().is_some_and(|a| !a.is_empty())
            || request.royalties.is_some()
            || request.reseller_cut.is_some()
        {
            return Err("a free op_type must not carry sale/royalty terms".to_string());
        }
    }
    Ok(())
}

fn require_non_empty(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    Ok(())
}

fn validate_decimal(value: &str, field: &str) -> Result<(), String> {
    if value.bytes().all(|b| b.is_ascii_digit()) && !value.is_empty() {
        Ok(())
    } else {
        Err(format!("{field} must be a base-10 integer string"))
    }
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
        "publish-provider: starting v{} (on-chain content mint assembly)",
        PROVIDER_VERSION
    );

    let mut provider = PublishProvider;
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("publish-provider read error: {err}");
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

    eprintln!("publish-provider exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    const KID: &str = "38691296765e76a331f5d5630bddf9f5";

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

    /// Drive the REAL parse->handle path so serde validation (unknown op_type, missing
    /// fields) is exercised the same way the stdin loop would.
    fn prepare(req: Value) -> Response {
        let request: Request = match serde_json::from_value(json!({
            "op": "prepare_publish",
            "request": req,
        })) {
            Ok(r) => r,
            Err(e) => return Response::error("invalid_request", e.to_string()),
        };
        PublishProvider.handle(request)
    }

    const CREATOR: &str = "0x1111111111111111111111111111111111111111";

    fn paid_request() -> Value {
        json!({
            "schema": PUBLISH_REQUEST_SCHEMA,
            "request_id": "publish:test",
            "kid_hex": KID,
            "metadata_cid": "QmMetaFolderCidV0",
            "channel_address": "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D",
            "op_type": "buy_once",
            "price_wei": "1000000000000000000",
            "copies": 100,
            "creator_address": CREATOR,
        })
    }

    #[test]
    fn prepare_binds_content_id_to_the_bytes16_kid() {
        let data = ok_data(prepare(paid_request()));
        assert_eq!(data["status"], "prepared");
        assert_eq!(data["content_id"], format!("0x{KID}"));
        assert_eq!(data["unsigned_mint"]["content_id"], format!("0x{KID}"));
        // The identity carried on-chain is exactly the producer's KID, 16 bytes.
        let id = data["content_id"].as_str().unwrap();
        assert_eq!(id.strip_prefix("0x").unwrap().len(), 32);
    }

    #[test]
    fn token_uri_is_the_metadata_folder_metadata_json() {
        let data = ok_data(prepare(paid_request()));
        assert_eq!(data["token_uri"], "QmMetaFolderCidV0/metadata.json");
        assert_eq!(
            data["unsigned_mint"]["token_uri"],
            "QmMetaFolderCidV0/metadata.json"
        );
    }

    #[test]
    fn unsigned_mint_mirrors_the_pc2_mint_signature_and_op_codes() {
        let data = ok_data(prepare(paid_request()));
        let mint = &data["unsigned_mint"];
        assert_eq!(mint["function"], "mint(string,uint16,bytes,bytes)");
        assert_eq!(mint["op_type"], "buy_once");
        assert_eq!(mint["op_type_code"], 1);
        assert_eq!(mint["chain_id"], 8453);
    }

    #[test]
    fn non_bytes16_kid_is_rejected() {
        // 31 hex chars.
        let mut req = paid_request();
        req["kid_hex"] = json!("38691296765e76a331f5d5630bddf9f");
        assert_eq!(error_code(prepare(req.clone())), "invalid_request");
        // non-hex.
        req["kid_hex"] = json!("zz691296765e76a331f5d5630bddf9f5");
        assert_eq!(error_code(prepare(req)), "invalid_request");
    }

    #[test]
    fn content_id_is_lowercased_like_pc2() {
        let mut req = paid_request();
        req["kid_hex"] = json!("38691296765E76A331F5D5630BDDF9F5");
        let data = ok_data(prepare(req));
        assert_eq!(data["content_id"], format!("0x{KID}"));
    }

    #[test]
    fn paid_publish_requires_a_price() {
        let mut req = paid_request();
        req.as_object_mut().unwrap().remove("price_wei");
        assert_eq!(error_code(prepare(req)), "invalid_request");
    }

    #[test]
    fn free_publish_must_not_carry_sale_terms() {
        let free = json!({
            "schema": PUBLISH_REQUEST_SCHEMA,
            "request_id": "publish:free",
            "kid_hex": KID,
            "metadata_cid": "QmMetaFolderCidV0",
            "channel_address": "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D",
            "op_type": "free",
            "price_wei": "1000",
        });
        assert_eq!(error_code(prepare(free)), "invalid_request");
    }

    #[test]
    fn free_publish_assembles_without_sale_terms() {
        let free = json!({
            "schema": PUBLISH_REQUEST_SCHEMA,
            "request_id": "publish:free",
            "kid_hex": KID,
            "metadata_cid": "QmMetaFolderCidV0",
            "channel_address": "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D",
            "op_type": "free",
        });
        let data = ok_data(prepare(free));
        assert_eq!(data["unsigned_mint"]["op_type_code"], 0);
        assert!(data["unsigned_mint"]["sell"].is_null());
    }

    #[test]
    fn invalid_channel_address_is_rejected() {
        let mut req = paid_request();
        req["channel_address"] = json!("not-an-address");
        assert_eq!(error_code(prepare(req)), "invalid_request");
    }

    #[test]
    fn unknown_op_type_is_rejected() {
        let mut req = paid_request();
        req["op_type"] = json!("rent");
        assert_eq!(error_code(prepare(req)), "invalid_request");
    }

    #[test]
    fn assembled_mint_is_unsigned_and_carries_no_signing_or_rpc_authority() {
        let data = ok_data(prepare(paid_request()));
        // It is explicitly unsigned, never "published", and defers the fee + broadcast.
        assert_eq!(data["unsigned_mint"]["signed"], false);
        assert_eq!(data["status"], "prepared");
        assert_ne!(data["status"], "published");
        assert!(data["unsigned_mint"]["value_wei"].is_null());
        assert_eq!(data["requires"][0], "chain-provider");
        assert_eq!(data["requires"][1], "wallet-provider");
        // No private key / signature material anywhere in the receipt.
        let serialized = serde_json::to_string(&data).unwrap();
        for forbidden in ["private_key", "signature", "mnemonic", "secret"] {
            assert!(
                !serialized.contains(forbidden),
                "publish receipt must not carry {forbidden}"
            );
        }
    }

    #[test]
    fn status_blocks_chain_and_wallet_authority() {
        let data = ok_data(PublishProvider.status());
        let blocked = data["blocked_authority"].as_array().unwrap();
        for must in ["chain_rpc", "wallet_keys", "broadcast"] {
            assert!(blocked.iter().any(|v| v == must), "must block {must}");
        }
        assert_eq!(data["next_required_providers"][0], "chain-provider");
        assert_eq!(data["next_required_providers"][1], "wallet-provider");
    }

    #[test]
    fn paid_publish_carries_assemble_mint_ready_sell_terms() {
        let data = ok_data(prepare(paid_request()));
        let sell = &data["unsigned_mint"]["sell"];
        // Shape EXACTLY matches chain-provider::assemble_mint's MintSell.
        assert_eq!(sell["price_wei"], "1000000000000000000");
        assert_eq!(sell["copies"], "100"); // decimal string for uint256
        assert_eq!(sell["pay_token"], ZERO_ADDRESS); // native default
    }

    #[test]
    fn paid_op_raw_mirrors_pc2_payee_arrays() {
        let data = ok_data(prepare(paid_request()));
        let op = &data["unsigned_mint"]["op_raw"];
        assert_eq!(op["metadata_uri"], "ipfs://QmMetaFolderCidV0");
        // [creator(ACCESS_TOKEN, copies), creator(ROYALTY_SHARE, round(10*95)=950)].
        assert_eq!(op["addresses"][0], CREATOR);
        assert_eq!(op["addresses"][1], CREATOR);
        assert_eq!(op["role_types"][0], ROLE_ACCESS_TOKEN);
        assert_eq!(op["role_types"][1], ROLE_ROYALTY_SHARE);
        assert_eq!(op["amounts"][0], "100");
        assert_eq!(op["amounts"][1], "950");
        // BUY_ONCE carries no resellerCut (it would shift the ABI layout).
        assert!(op["reseller_cut"].is_null());
    }

    #[test]
    fn buy_and_resell_appends_reseller_cut_and_distribution_right() {
        let mut req = paid_request();
        req["op_type"] = json!("buy_and_resell");
        req["royalties"] = json!([
            { "address": CREATOR, "royalty": 90.0, "identifier": "A" },
            { "address": "0x2222222222222222222222222222222222222222", "royalty": 5.0, "identifier": "C" },
        ]);
        let data = ok_data(prepare(req));
        let op = &data["unsigned_mint"]["op_raw"];
        assert_eq!(op["reseller_cut"], DEFAULT_RESELLER_CUT);
        // creator(ACCESS) + 2 royalties + distributor(DISTRIBUTION_RIGHT).
        assert_eq!(op["role_types"].as_array().unwrap().len(), 4);
        assert_eq!(op["role_types"][3], ROLE_DISTRIBUTION_RIGHT);
        assert_eq!(op["amounts"][3], "1");
    }

    #[test]
    fn paid_publish_requires_a_creator_address() {
        let mut req = paid_request();
        req.as_object_mut().unwrap().remove("creator_address");
        assert_eq!(error_code(prepare(req)), "invalid_request");
    }
}
