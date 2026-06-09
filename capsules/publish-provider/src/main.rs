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
}

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
        // left STRUCTURED (not raw calldata) so the EVM specialist (chain-provider) owns
        // the encoding and publish-provider stays capability-clean.
        let mut sell = json!(null);
        if request.op_type.is_paid() {
            sell = json!({
                "price_wei": request.price_wei,
                "currency_address": request.currency_address,
                "copies": request.copies.unwrap_or(1),
            });
        }
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
        {
            return Err("a free op_type must not carry price/currency/copies".to_string());
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
    fn paid_publish_carries_structured_sale_terms() {
        let data = ok_data(prepare(paid_request()));
        let sell = &data["unsigned_mint"]["sell"];
        assert_eq!(sell["price_wei"], "1000000000000000000");
        assert_eq!(sell["copies"], 100);
    }
}
