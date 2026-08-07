//! dDRM producer→chain→discovery market smoke (Phase C, Day 64).
//!
//! Drives the REAL `publish-provider`, `chain-provider`, and `content-market` binaries
//! over their newline-delimited JSON protocol to prove the full producer→discovery seam:
//!
//!   publish (prepare_publish) -> chain (assemble_mint) -> content-market (reconstruct_listing)
//!
//! `publish-provider` binds `contentId == bytes16 KID`; `chain-provider` ABI-encodes the
//! PC2 `mint(...)` calldata; `content-market` decodes THAT SAME calldata back into a
//! `ContentListingV1`. The smoke asserts ONE identity survives every hop: the listing's
//! `content_id` equals the contentId publish bound equals `0x{KID}` — and that the
//! discovery step neither mints, signs, nor touches RPC/IPFS. PAID and FREE both flow.
//!
//! Usage: ddrm-market-smoke <publish-bin> <chain-bin> <content-market-bin>

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const KID: &str = "38691296765e76a331f5d5630bddf9f5";
const CHANNEL: &str = "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D";
const CREATOR: &str = "0x1111111111111111111111111111111111111111";
const SELECTOR: &str = "0xaabbccdd";
const META_CID: &str = "QmMetaFolderCidV0";
const EVENT_HUB: &str = "0x3333333333333333333333333333333333333333";
// keccak256("DigitalAssetRegistered(address,uint256,address,string,uint16,bytes16)").
const TOPIC_DAR: &str = "0x1b24f7763272894608506beba5887c374d345cd231bf52bd03f40bc2d0508d7b";

struct Capsule {
    name: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Capsule {
    fn spawn(name: &str, bin: &str) -> Result<Self, String> {
        let mut child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn {name} ({bin}): {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        Ok(Self {
            name: name.to_string(),
            child,
            stdin,
            stdout,
        })
    }

    fn call(&mut self, req: &Value) -> Result<Value, String> {
        let line = serde_json::to_string(req).map_err(|e| e.to_string())?;
        writeln!(self.stdin, "{line}").map_err(|e| format!("write to {}: {e}", self.name))?;
        self.stdin.flush().map_err(|e| e.to_string())?;
        let mut resp = String::new();
        let n = self
            .stdout
            .read_line(&mut resp)
            .map_err(|e| format!("read from {}: {e}", self.name))?;
        if n == 0 {
            return Err(format!("{} closed its output unexpectedly", self.name));
        }
        serde_json::from_str(resp.trim())
            .map_err(|e| format!("{} sent non-JSON: {e}: {resp}", self.name))
    }

    fn shutdown(mut self) {
        let _ = self.call(&json!({ "op": "shutdown" }));
        let _ = self.child.wait();
    }
}

fn ok_data(resp: &Value, ctx: &str) -> Result<Value, String> {
    if resp.get("status").and_then(Value::as_str) == Some("ok") {
        Ok(resp.get("data").cloned().unwrap_or(Value::Null))
    } else {
        Err(format!("{ctx}: expected ok, got {resp}"))
    }
}

fn step(n: u32, msg: &str) {
    println!("  [{n}] {msg}");
}

fn assemble_mint_request(unsigned_mint: &Value) -> Value {
    let mut mint = json!({
        "selector": SELECTOR,
        "to": unsigned_mint["to"],
        "token_uri": unsigned_mint["token_uri"],
        "op_type_code": unsigned_mint["op_type_code"],
        "content_id": unsigned_mint["content_id"],
    });
    if !unsigned_mint["op_raw"].is_null() {
        mint["op_raw"] = unsigned_mint["op_raw"].clone();
    }
    if !unsigned_mint["sell"].is_null() {
        mint["sell"] = unsigned_mint["sell"].clone();
    }
    json!({ "op": "assemble_mint", "mint": mint })
}

fn reconstruct_request(calldata: &str) -> Value {
    json!({
        "op": "reconstruct_listing",
        "request": {
            "calldata": calldata,
            "channel_address": CHANNEL,
            "chain_id": 8453,
            "expected_selector": SELECTOR,
        }
    })
}

fn enrich_request(calldata: &str, kid: &str) -> Value {
    json!({
        "op": "enrich_listing",
        "request": {
            "calldata": calldata,
            "channel_address": CHANNEL,
            "chain_id": 8453,
            "expected_selector": SELECTOR,
            "metadata": {
                "schema": "elacity-asset-envelope-v1",
                "name": "Market Smoke Film",
                "description": "Sealed now, discoverable now.",
                "image": "ipfs://QmPoster/poster.png",
                "kid": kid,
                "media": {
                    "uri": "ipfs://QmContent/video.mp4",
                    "contentType": "video/mp4"
                }
            }
        }
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

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

fn address_word(addr: &str) -> Vec<u8> {
    let clean = addr.trim_start_matches("0x");
    let raw: Vec<u8> = (0..clean.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).unwrap())
        .collect();
    let mut w = vec![0u8; 32];
    w[12..32].copy_from_slice(&raw);
    w
}

fn bytes16_word(kid_hex: &str) -> Vec<u8> {
    let raw: Vec<u8> = (0..kid_hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&kid_hex[i..i + 2], 16).unwrap())
        .collect();
    let mut w = vec![0u8; 32];
    w[0..16].copy_from_slice(&raw[0..16]);
    w
}

fn dyn_string(s: &str) -> Vec<u8> {
    let payload = s.as_bytes();
    let mut out = pad_left_u64(payload.len() as u64);
    out.extend_from_slice(payload);
    let rem = payload.len() % 32;
    if rem != 0 {
        out.extend(std::iter::repeat(0u8).take(32 - rem));
    }
    out
}

/// Build a DigitalAssetRegistered log carrying our contentId: topics[1]=channel,
/// data = abi.encode(creator, tokenURI, opType, contentId).
fn dar_event_request(token_uri: &str, op_code: u16) -> Value {
    let mut data = address_word(CREATOR);
    data.extend(pad_left_u64(4 * 32)); // string offset
    data.extend(pad_left_u16(op_code));
    data.extend(bytes16_word(KID));
    data.extend(dyn_string(token_uri));
    json!({
        "op": "listing_from_event",
        "request": {
            "topics": [
                TOPIC_DAR,
                format!("0x{}", hex(&address_word(CHANNEL))),
                format!("0x{}", hex(&pad_left_u64(1))),
            ],
            "data": format!("0x{}", hex(&data)),
            "address": EVENT_HUB,
            "chain_id": 8453,
        }
    })
}

fn publish_request(op_type: &str, paid: bool) -> Value {
    let mut req = json!({
        "schema": "elastos.publish.request/v1",
        "request_id": format!("market-smoke:{op_type}"),
        "kid_hex": KID,
        "metadata_cid": META_CID,
        "channel_address": CHANNEL,
        "op_type": op_type,
    });
    if paid {
        req["price_wei"] = json!("1000000000000000000");
        req["copies"] = json!(100);
        req["creator_address"] = json!(CREATOR);
    }
    json!({ "op": "prepare_publish", "request": req })
}

/// Drive publish -> chain -> content-market for one op_type and assert the identity holds.
fn flow_one(
    op_type: &str,
    paid: bool,
    publish: &mut Capsule,
    chain: &mut Capsule,
    market: &mut Capsule,
) -> Result<(), String> {
    let prepared = ok_data(&publish.call(&publish_request(op_type, paid))?, "publish prepare")?;
    let content_id = prepared["content_id"].as_str().ok_or("no content_id")?.to_string();

    let assembled = ok_data(
        &chain.call(&assemble_mint_request(&prepared["unsigned_mint"]))?,
        "chain assemble_mint",
    )?;
    let calldata = assembled["data"].as_str().ok_or("no calldata")?.to_string();

    let listing = ok_data(
        &market.call(&reconstruct_request(&calldata))?,
        "content-market reconstruct_listing",
    )?;

    // ONE identity across all three: publish.contentId == listing.content_id == 0x{KID}.
    if listing["content_id"].as_str() != Some(&content_id) {
        return Err(format!("contentId drifted into the listing: {listing}"));
    }
    if listing["content_id"].as_str() != Some(&format!("0x{KID}")) {
        return Err("listing content_id is not the producer's KID".to_string());
    }
    if listing["op_type"].as_str() != Some(op_type) {
        return Err(format!("op_type drifted: {listing}"));
    }
    if listing["metadata_cid"].as_str() != Some(META_CID) {
        return Err(format!("metadata CID not recovered from tokenURI: {listing}"));
    }
    if paid {
        if listing["price_wei"].as_str() != Some("1000000000000000000") {
            return Err(format!("sell price did not survive the round trip: {listing}"));
        }
        if listing["copies"].as_str() != Some("100") {
            return Err(format!("copies did not survive the round trip: {listing}"));
        }
    }
    // Discovery is read-only: a listing reconstructed from calldata, not minted.
    if listing["source"].as_str() != Some("mint_calldata") {
        return Err("listing provenance is not the mint calldata".to_string());
    }

    // Enrich with a matching metadata.json -> resolved card; identity unchanged.
    let resolved = ok_data(
        &market.call(&enrich_request(&calldata, KID))?,
        "content-market enrich_listing",
    )?;
    if resolved["content_id"].as_str() != Some(&content_id) {
        return Err("enrichment changed the listing identity".to_string());
    }
    if resolved["metadata_status"].as_str() != Some("resolved") {
        return Err(format!("enrichment did not resolve: {resolved}"));
    }
    if resolved["name"].as_str() != Some("Market Smoke Film")
        || resolved["asset_type"].as_str() != Some("video")
    {
        return Err(format!("descriptive fields not fused: {resolved}"));
    }

    // A tampered kid must NEVER re-point the listing.
    let tampered = market.call(&enrich_request(&calldata, "ffffffffffffffffffffffffffffffff"))?;
    if tampered.get("status").and_then(Value::as_str) != Some("error")
        || tampered.get("code").and_then(Value::as_str) != Some("identity_mismatch")
    {
        return Err(format!("tampered metadata.kid was not rejected: {tampered}"));
    }

    // The chain's own emitted log agrees with what we assembled: a DigitalAssetRegistered
    // event reconstructs the SAME identity as the calldata path.
    let op_code = listing["op_type_code"].as_u64().ok_or("no op_type_code")? as u16;
    let token_uri = listing["token_uri"].as_str().ok_or("no token_uri")?;
    let from_event = ok_data(
        &market.call(&dar_event_request(token_uri, op_code))?,
        "content-market listing_from_event",
    )?;
    if from_event["content_id"].as_str() != Some(&content_id) {
        return Err(format!("event path disagreed with calldata on identity: {from_event}"));
    }
    if from_event["token_uri"].as_str() != Some(token_uri)
        || from_event["op_type"].as_str() != listing["op_type"].as_str()
        || from_event["source"].as_str() != Some("chain_event")
    {
        return Err(format!("event listing did not match calldata listing: {from_event}"));
    }

    step(
        if paid { 1 } else { 2 },
        &format!(
            "{op_type}: KID -> calldata -> listing -> enrich(resolved); tampered kid rejected; chain event agrees; content_id={content_id} intact"
        ),
    );
    Ok(())
}

fn run(args: &[String]) -> Result<(), String> {
    let publish_bin = args.first().ok_or("missing <publish-bin>")?;
    let chain_bin = args.get(1).ok_or("missing <chain-bin>")?;
    let market_bin = args.get(2).ok_or("missing <content-market-bin>")?;

    println!("== dDRM market smoke (publish -> chain -> content-market) ==");

    let mut publish = Capsule::spawn("publish-provider", publish_bin)?;
    let mut chain = Capsule::spawn("chain-provider", chain_bin)?;
    let mut market = Capsule::spawn("content-market", market_bin)?;

    flow_one("buy_once", true, &mut publish, &mut chain, &mut market)?;
    flow_one("free", false, &mut publish, &mut chain, &mut market)?;

    publish.shutdown();
    chain.shutdown();
    market.shutdown();

    println!();
    println!("RESULT: a sealed asset's KID flowed producer -> chain -> discovery as ONE identity:");
    println!("        the listing's content_id IS the producer's KID, reconstructed from the");
    println!("        self-describing mint calldata. No minting/signing/RPC in the discovery step.");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("ddrm-market-smoke: {e}");
            std::process::exit(1);
        }
    }
}
