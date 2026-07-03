//! Marketplace READ views — pure ABI encoders/decoders + thin live wrappers for the gateway's
//! discovery/detail reads. Today: `sellersOf(operative, tokenId) -> address[]` (the listing's sellers).
//! Terms (`listings`) are read via `buy_authority::read_listing_terms` (one canonical path, P10). All
//! chain access is a READ-ONLY `eth_call` through the sole RPC declarant (chain-provider via `chain_tx`);
//! this module holds no keys/RPC/signer of its own (P3). Selectors are keccak-pinned + confirmed in
//! CONTRACTS.md. Every decoder is fail-soft (Err, never panic) on malformed/hostile RPC data.

/// `sellersOf(address op, uint256 tokenId) -> address[]` — pinned in CONTRACTS.md §3.4.
const SELLERS_OF_SELECTOR: &str = "997eab2d";

/// A 20-byte EVM address (`0x`+40 hex) left-padded into a 32-byte ABI word (64 hex). `None` if not an
/// address — so a caller fails closed rather than encoding garbage into calldata.
fn address_word(addr: &str) -> Option<String> {
    let h = addr
        .strip_prefix("0x")
        .or_else(|| addr.strip_prefix("0X"))
        .unwrap_or(addr);
    if h.len() != 40 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("{:0>64}", h.to_lowercase()))
}

/// Parse a 32-byte ABI word (64 hex) as a `usize` (low 8 bytes; high 24 must be zero). Fail-closed.
fn word_to_usize(word: &str) -> Result<usize, String> {
    if word.len() != 64 || !word.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("not a 32-byte hex word".to_string());
    }
    if word[..48].bytes().any(|b| b != b'0') {
        return Err("ABI word exceeds usize range".to_string());
    }
    usize::from_str_radix(&word[48..], 16).map_err(|e| e.to_string())
}

/// Encode `sellersOf(operative, tokenId)` calldata. `token_id_word` is the 64-hex (no `0x`) token id
/// (`buy_authority::token_id_to_word` output). Fail-closed on a bad address / non-word tokenId.
pub(crate) fn encode_sellers_of(operative: &str, token_id_word: &str) -> Result<String, String> {
    let op = address_word(operative)
        .ok_or_else(|| format!("operative is not an address: {operative}"))?;
    let tid = token_id_word.trim().trim_start_matches("0x");
    if tid.len() != 64 || !tid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("token_id is not a 32-byte word: {token_id_word}"));
    }
    Ok(format!("0x{SELLERS_OF_SELECTOR}{op}{}", tid.to_lowercase()))
}

/// Decode an ABI dynamic `address[]` return. Fail-soft (`Err`, never panic) on any malformed/hostile
/// offset/length/word: a hostile RPC word can be up to `u64::MAX`, so every index is checked and the
/// element count is capped at `MAX_SELLERS` before allocation.
pub(crate) fn decode_address_array(result: &str) -> Result<Vec<String>, String> {
    const MAX_SELLERS: usize = 256;
    let clean = result.trim().trim_start_matches("0x");
    if !clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("address[] return is not hex".to_string());
    }
    // word 0 = byte-offset to the array data; word at that offset = length; then `length` address words.
    let off = word_to_usize(
        clean
            .get(0..64)
            .ok_or("address[] return too short for offset")?,
    )?;
    let len_at = off.checked_mul(2).ok_or("address[] offset overflow")?;
    let len_end = len_at.checked_add(64).ok_or("address[] length overflow")?;
    let len = word_to_usize(
        clean
            .get(len_at..len_end)
            .ok_or("address[] return too short for length")?,
    )?;
    if len > MAX_SELLERS {
        return Err(format!(
            "address[] length {len} exceeds bound {MAX_SELLERS}"
        ));
    }
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let at = len_end
            .checked_add(i.checked_mul(64).ok_or("address[] index overflow")?)
            .ok_or("address[] index overflow")?;
        let end = at.checked_add(64).ok_or("address[] word overflow")?;
        let word = clean
            .get(at..end)
            .ok_or("address[] return too short for element")?;
        out.push(format!("0x{}", &word[24..])); // address = last 20 bytes (40 hex) of the word
    }
    Ok(out)
}

/// Live `sellersOf(operative, ACCESS_TOKEN=1)` via the gateway view (chain-provider `eth_call`).
/// READ-ONLY. `sellersOf` (like `listings`) is keyed at the ERC-1155 ACCESS_TOKEN id (== 1), NOT the
/// asset content tokenId (CONTRACTS.md §1.3; confirmed live — a content-tokenId read returns empty), so
/// the caller's `_token_id_word` is irrelevant to this read.
pub(crate) fn sellers_of_live(
    gateway: &str,
    operative: &str,
    _token_id_word: &str,
) -> Result<Vec<String>, String> {
    let data = encode_sellers_of(operative, super::buy_authority::ACCESS_TOKEN_ID_WORD)?;
    let result = super::chain_tx::contract_call_live(gateway, &data)?;
    decode_address_array(&result)
}

// ---- listing enrichment: descriptive fields parsed from an asset `metadata.json` ----
// The same field paths content-market's `enrich_listing` reads (PC2 ContentIndexerService.ts:1102). PURE —
// the caller fetches the JSON (live, via the content/* plane); these parse it. `content_cid` (from
// `media.uri`) is the ENCRYPTED asset CID the buy->pin needs. NOTE: this is the descriptive slice; the full
// content-market enrich additionally validates `metadata.kid == calldata contentId` (hardening follow-on).

/// Resolve the IPFS CID from a metadata/token URI — content-market's rule (`ipfs://`, `/ipfs/`, bare Qm/bafy).
pub(crate) fn extract_cid(uri: &str) -> Option<String> {
    if let Some(rest) = uri.strip_prefix("ipfs://") {
        return rest
            .split('/')
            .next()
            .filter(|s| !s.is_empty())
            .map(str::to_string);
    }
    if let Some(idx) = uri.find("/ipfs/") {
        return uri[idx + 6..]
            .split('/')
            .next()
            .filter(|s| !s.is_empty())
            .map(str::to_string);
    }
    if uri.starts_with("Qm") || uri.starts_with("bafy") {
        return uri
            .split('/')
            .next()
            .filter(|s| !s.is_empty())
            .map(str::to_string);
    }
    None
}

/// The in-CID path of a token/metadata URI, if any. Mints publish `metadata.json` as a UnixFS *directory*, so
/// the tokenURI is `ipfs://<dirCid>/metadata.json` — the bare CID alone resolves to the directory, not the
/// JSON. Returns `Some("metadata.json")` for that shape; `None` when the URI is a bare single-file CID.
pub(crate) fn extract_cid_subpath(uri: &str) -> Option<String> {
    let rest = uri
        .strip_prefix("ipfs://")
        .map(str::to_string)
        .or_else(|| uri.find("/ipfs/").map(|i| uri[i + 6..].to_string()))
        .or_else(|| (uri.starts_with("Qm") || uri.starts_with("bafy")).then(|| uri.to_string()))?;
    let (_, path) = rest.split_once('/')?;
    (!path.is_empty()).then(|| path.to_string())
}

/// Descriptive fields from an asset `metadata.json` (name/cover + the encrypted `content_cid` + mime),
/// plus `kid` — the bytes16 contentId the asset metadata claims (content-market validates this against the
/// calldata; we use it to bind the acquire's pinned CID to the gated KID).
#[derive(Debug, Default, serde::Serialize)]
pub(crate) struct AssetMeta {
    pub name: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub content_cid: Option<String>,
    pub mime_type: Option<String>,
    pub kid: Option<String>,
    /// The metadata `attributes[]` (trait_type/value) — the heart of the asset's Properties panel.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<AssetTrait>,
    /// Curated scalar `properties` (usage rights, label, authority, chain, publisher) for the Properties panel.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<AssetTrait>,
    /// Encrypted media size in bytes (from `media.size`), for the "File size" property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_size: Option<u64>,
    /// Human content type from `media.contentType` (e.g. "Video") — nicer than the raw MIME.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// `media.previewURL` (often a DASH `.mpd`) — drives the "Preview available" indicator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    /// `createdAt` (ISO-8601) from the metadata — the "Uploaded · N ago" line (no chain read needed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// `properties.categories` (e.g. "Science & Technology") — discovery chips, mirrors elacity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    /// `properties.tags` (e.g. "post-quantum") — discovery chips, mirrors elacity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// A label/value pair surfaced in the Properties panel — used for both metadata `attributes[]`
/// (`trait_type`/`value`) and curated `properties` scalars (the key is the property name).
#[derive(Debug, serde::Serialize)]
pub(crate) struct AssetTrait {
    pub label: String,
    pub value: String,
}

/// Curated `properties` scalars surfaced as Properties rows (mirrors elacity-web `MediaProperties`).
const PROPERTY_KEYS: &[&str] = &[
    "distribution",
    "labelType",
    "authority",
    "chainId",
    "publisher",
    "symbol",
];

/// Stringify a JSON scalar (string/number/bool) for display; `None` for empty strings, null, arrays, objects.
fn scalar_to_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Parse the descriptive fields from a fetched `metadata.json`. PURE; mirrors content-market's paths.
pub(crate) fn parse_asset_metadata(meta: &serde_json::Value) -> AssetMeta {
    let s = |v: Option<&serde_json::Value>| {
        v.and_then(serde_json::Value::as_str)
            .filter(|x| !x.is_empty())
            .map(str::to_string)
    };
    let media = meta.get("media");
    let attributes = meta
        .get("attributes")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|it| {
                    let label = it
                        .get("trait_type")
                        .and_then(serde_json::Value::as_str)
                        .filter(|t| !t.trim().is_empty())?;
                    let value = it.get("value").and_then(scalar_to_string)?;
                    Some(AssetTrait {
                        label: label.trim().to_string(),
                        value,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let properties = meta
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|obj| {
            PROPERTY_KEYS
                .iter()
                .filter_map(|k| {
                    obj.get(*k)
                        .and_then(scalar_to_string)
                        .map(|value| AssetTrait {
                            label: (*k).to_string(),
                            value,
                        })
                })
                .collect()
        })
        .unwrap_or_default();
    let media_size = media.and_then(|m| m.get("size")).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    });
    let str_array = |key: &str| {
        meta.get("properties")
            .and_then(|p| p.get(key))
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        v.as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    AssetMeta {
        name: s(meta.get("name")),
        description: s(meta.get("description")),
        image_url: s(meta.get("image")).or_else(|| s(media.and_then(|m| m.get("previewURL")))),
        content_cid: media
            .and_then(|m| m.get("uri"))
            .and_then(serde_json::Value::as_str)
            .and_then(extract_cid),
        mime_type: s(media.and_then(|m| m.get("contentType"))),
        kid: s(meta.get("kid")).or_else(|| s(meta.get("properties").and_then(|p| p.get("kid")))),
        attributes,
        properties,
        media_size,
        content_type: s(media.and_then(|m| m.get("contentType"))),
        preview_url: s(media.and_then(|m| m.get("previewURL"))),
        created_at: s(meta.get("createdAt")),
        categories: str_array("categories"),
        tags: str_array("tags"),
    }
}

/// One playable track of a clear (unencrypted) DASH preview: the codec MIME for MSE plus the in-CID
/// init + ordered segment paths. The shell appends these into an MSE SourceBuffer (mirrors elacity-player).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct PreviewTrack {
    pub kind: String,
    pub mime: String,
    pub init_path: String,
    pub seg_paths: Vec<String>,
}

/// Read one XML attribute value (`name="value"`) out of a tag/blob. Returns the first match. PURE.
fn xml_attr(s: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let start = s.find(&key)? + key.len();
    let rest = &s[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Parse a Bento4-style clear DASH manifest (`SegmentTemplate` + `SegmentTimeline`) into one track per
/// AdaptationSet, choosing the lowest-bandwidth video representation (a preview only needs to look right)
/// and the single audio representation. Fail-SOFT: returns whatever it could parse (`[]` on garbage), so a
/// hostile/odd manifest degrades to "no preview" rather than erroring. Encrypted manifests are NOT handled
/// here — the caller only feeds the public `previewURL`, which carries no `<ContentProtection>`.
pub(crate) fn parse_dash_preview(xml: &str) -> Vec<PreviewTrack> {
    let mut tracks = Vec::new();
    let mut sets = xml.split("<AdaptationSet");
    let _ = sets.next(); // preamble before the first set
    for raw in sets {
        let block = raw.split("</AdaptationSet>").next().unwrap_or(raw);
        let mime_type = match xml_attr(block, "mimeType") {
            Some(m) => m,
            None => continue,
        };
        let kind = if mime_type.starts_with("video") {
            "video"
        } else if mime_type.starts_with("audio") {
            "audio"
        } else {
            continue;
        };
        let tmpl = match block.find("<SegmentTemplate") {
            Some(i) => &block[i..],
            None => continue,
        };
        let init_tmpl = match xml_attr(tmpl, "initialization") {
            Some(v) => v,
            None => continue,
        };
        let media_tmpl = match xml_attr(tmpl, "media") {
            Some(v) => v,
            None => continue,
        };
        let start_number: u64 = xml_attr(tmpl, "startNumber")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        // Count timeline entries: each `<S ...>` is one segment plus its `r` additional repeats.
        let mut seg_count: u64 = 0;
        for (idx, _) in block.match_indices("<S ") {
            let stag = block[idx..].split('>').next().unwrap_or("");
            let r = xml_attr(stag, "r")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            seg_count += 1 + r;
        }
        if seg_count == 0 {
            continue;
        }
        // Pick a representation: lowest-bandwidth for video (smallest/fastest preview), first for audio.
        let mut chosen: Option<(u64, String, String)> = None; // (bandwidth, id, codecs)
        for rep in block.split("<Representation").skip(1) {
            let rtag = rep.split('>').next().unwrap_or("");
            let id = match xml_attr(rtag, "id") {
                Some(v) => v,
                None => continue,
            };
            let codecs = xml_attr(rtag, "codecs").unwrap_or_default();
            let bw = xml_attr(rtag, "bandwidth")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let take = match &chosen {
                None => true,
                Some((cbw, _, _)) => kind == "video" && bw < *cbw,
            };
            if take {
                chosen = Some((bw, id, codecs));
            }
        }
        let (_, id, codecs) = match chosen {
            Some(c) => c,
            None => continue,
        };
        let mime = if codecs.is_empty() {
            mime_type.clone()
        } else {
            format!("{mime_type}; codecs=\"{codecs}\"")
        };
        let init_path = init_tmpl.replace("$RepresentationID$", &id);
        let seg_paths = (start_number..start_number + seg_count)
            .map(|n| {
                media_tmpl
                    .replace("$RepresentationID$", &id)
                    .replace("$Number$", &n.to_string())
            })
            .collect();
        tracks.push(PreviewTrack {
            kind: kind.to_string(),
            mime,
            init_path,
            seg_paths,
        });
    }
    tracks
}

/// Normalize a bytes16 contentId/KID for comparison: lowercase, no `0x`. `None` if not 32 clean hex.
pub(crate) fn normalize_kid(raw: &str) -> Option<String> {
    let h = raw
        .trim()
        .strip_prefix("0x")
        .or_else(|| raw.trim().strip_prefix("0X"))
        .unwrap_or(raw.trim());
    if h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(h.to_lowercase())
    } else {
        None
    }
}

// ---- hasAccessByContentId(address holder, bytes16 contentId) -> bool (the "do I own this" read) ----
// Selector + ABI layout match chain-provider::encode_has_access_by_content_id_address_bytes16 (the holder
// address word, then the bytes16 KID LEFT-aligned). Gateway-side read via the sole RPC declarant.
const HAS_ACCESS_SELECTOR: &str = "54d42821";

/// Encode `hasAccessByContentId(holder, kid)` calldata. `kid` is the bytes16 KID (left-aligned per ABI).
pub(crate) fn encode_has_access(holder: &str, kid: &str) -> Result<String, String> {
    let h = address_word(holder).ok_or_else(|| format!("holder is not an address: {holder}"))?;
    let k = normalize_kid(kid).ok_or_else(|| format!("not a bytes16 KID: {kid}"))?;
    // bytes16 is LEFT-aligned: the 16 KID bytes (32 hex) in the high half, zero-padded RIGHT to 64 hex.
    Ok(format!("0x{HAS_ACCESS_SELECTOR}{h}{k:0<64}"))
}

/// Decode an ABI `bool` return — true iff a clean, non-zero 32-byte word. Fail-soft (false) on malformed.
pub(crate) fn decode_bool(result: &str) -> bool {
    let clean = result.trim().trim_start_matches("0x");
    !clean.is_empty()
        && clean.bytes().all(|b| b.is_ascii_hexdigit())
        && clean.bytes().any(|b| b != b'0')
}

/// Live `hasAccessByContentId(holder, kid)` via the gateway view (chain-provider eth_call). READ-ONLY.
pub(crate) fn has_access_live(gateway: &str, holder: &str, kid: &str) -> Result<bool, String> {
    let data = encode_has_access(holder, kid)?;
    let result = super::chain_tx::contract_call_live(gateway, &data)?;
    Ok(decode_bool(&result))
}

/// Batched `hasAccessByContentId(holder, kid)` for many KIDs against the AuthorityGateway view, via ONE
/// Multicall3 `aggregate3` round-trip (the Vault's "what do I hold" sweep — minted OR bought — without an
/// indexer; same canonical read as `has_access_live`, P10, only the transport is batched). Returns a
/// `Vec<bool>` aligned to `kids`. Fail-CLOSED: a reverting/short sub-call decodes to `false` (never
/// fabricated ownership), and a length mismatch is an error so the caller never zips misaligned results.
/// READ-ONLY; declares no funds, holds no keys.
pub(crate) fn has_access_batched(
    gateway: &str,
    holder: &str,
    kids: &[String],
) -> Result<Vec<bool>, String> {
    if kids.is_empty() {
        return Ok(Vec::new());
    }
    let calls: Vec<(String, String)> = kids
        .iter()
        .map(|kid| encode_has_access(holder, kid).map(|data| (gateway.to_string(), data)))
        .collect::<Result<_, _>>()?;
    let ret = super::chain_tx::contract_call_live(MULTICALL3, &encode_aggregate3(&calls)?)?;
    let res = decode_aggregate3(&ret)?;
    if res.len() != kids.len() {
        return Err(format!(
            "aggregate3 hasAccess result count mismatch: got {}, want {}",
            res.len(),
            kids.len()
        ));
    }
    Ok(res
        .into_iter()
        .map(|(ok, data)| ok && decode_bool(&data))
        .collect())
}

/// Batched `listings(operative, seller)` for many operatives via ONE Multicall3 `aggregate3` — the
/// seller's CURRENT resale listing on each asset (the Vault's "Listed" view). elacity reads this from its
/// subgraph; the runtime reads it DIRECTLY off the per-asset operative via the SAME `listings` decode the
/// buy path binds to (P10), no indexer (P5/P13). Each sub-call targets its own operative. Returns a Vec
/// aligned to `operatives`: `Some((supply, price_minor_units, pay_token))` when the seller has an ACTIVE
/// row (supply > 0), else `None`. Fail-CLOSED: a reverting/short/sold-out sub-call -> `None` (never a
/// fabricated listing). Length mismatch is an error so the caller never zips misaligned results.
/// One asset's active resale row for `my_listings_batched`: `(remaining_supply, price_minor_units,
/// pay_token)`, or `None` when the seller has no active listing on that operative.
pub(crate) type SellerListingRow = Option<(u128, String, String)>;

pub(crate) fn my_listings_batched(
    seller: &str,
    operatives: &[String],
) -> Result<Vec<SellerListingRow>, String> {
    if operatives.is_empty() {
        return Ok(Vec::new());
    }
    let calls: Vec<(String, String)> = operatives
        .iter()
        .map(|op| {
            (
                op.clone(),
                super::buy_authority::encode_listings(op, seller),
            )
        })
        .collect();
    let ret = super::chain_tx::contract_call_live(MULTICALL3, &encode_aggregate3(&calls)?)?;
    let res = decode_aggregate3(&ret)?;
    if res.len() != operatives.len() {
        return Err(format!(
            "aggregate3 listings result count mismatch: got {}, want {}",
            res.len(),
            operatives.len()
        ));
    }
    Ok(res
        .into_iter()
        .map(|(ok, data)| {
            if !ok {
                return None;
            }
            // The listing is keyed at the ERC-1155 ACCESS_TOKEN id (== 1), not the content tokenId.
            super::buy_authority::decode_listing_return(
                &data,
                seller,
                super::buy_authority::ACCESS_TOKEN_ID_WORD,
            )
            .ok()
            .and_then(|(terms, supply)| {
                (supply > 0).then_some((supply, terms.price, terms.pay_token))
            })
        })
        .collect())
}

// ---- per-asset Operative royalty/economics reads (the REAL, on-chain splits) ----
// elacity-web sources these from its centralized subgraph; the runtime reads them DIRECTLY from the
// per-asset operative + CoreStorage via eth_call, so no third-party indexer sits in the trust path
// (P5/P13). Selectors are keccak-pinned and cross-validated against the on-chain-confirmed
// `paymentProcessor() 0xf1c6bdf8` + the EIP-2981 `royaltyInfo(uint256,uint256) 0x2a55205a`. Every
// decoder is fail-closed (Err, never panic) on a revert / short / hostile return, so the caller HIDES
// the splits panel (P11) rather than ever showing a fabricated percentage.

/// `royaltyInfo(uint256 salePrice) -> (address receiver, uint256 amount)[]` on the Operative.
const ROYALTY_INFO_SELECTOR: &str = "cef6d368";
/// `resellerCut() -> uint16` on OperativeBuyableSellable (BUY_AND_RESELL only); 1000-scale (`/10` -> %).
const RESELLER_CUT_SELECTOR: &str = "d9286e58";
/// `protocolShares() -> (uint256 shares, address recipient)` on CoreStorage; 1000-scale (`/10` -> %).
const PROTOCOL_SHARES_SELECTOR: &str = "5ad62049";

/// The synthetic salePrice fed to `royaltyInfo` to read each receiver's share as a percentage:
/// `pct = amount / SALE_BASE * 100`. 1e12 keeps the contract's integer share math exact (per-1000 shares
/// of 1e12 stay whole), so the recovered percentages are not rounded by the on-chain division.
const ROYALTY_SALE_BASE: u128 = 1_000_000_000_000;

/// Parse a 32-byte ABI word (64 hex, no `0x`) as a `u128`. Fail-closed if the high 128 bits are set
/// (a value beyond u128 is absurd for these reads — treat as malformed/hostile).
fn word_to_u128(word: &str) -> Result<u128, String> {
    if word.len() != 64 || !word.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("not a 32-byte hex word".to_string());
    }
    if word[..32].bytes().any(|b| b != b'0') {
        return Err("uint exceeds u128 (malformed word)".to_string());
    }
    u128::from_str_radix(&word[32..], 16).map_err(|e| e.to_string())
}

/// Convert a 1000-scale share (the units `resellerCut` / `protocolShares` store) to a human percentage —
/// elacity-web canonical (`raw/10`): the mint validates `resellerCut` in `[100, 9000]` == `[10%, 90%]`.
fn share_per_1000_to_pct(raw: u128) -> f64 {
    raw as f64 / 10.0
}

// ---- listing-lifecycle EVENT topics (the Activity/History source) ----
// topic0 = keccak of the canonical v3 ABI signature; CONFIRMED live against deployed AuthorityGateway logs
// (CONTRACTS.md §1.1, 2026-06-23). All three events emit from the AuthorityGateway and carry the operative
// (`op`) at indexed topic2 and the tokenId at topic3 — so one getLogs filtered by topic2=op returns an
// asset's whole trade history. elacity-web reads these from its subgraph; the runtime reads the logs
// DIRECTLY (no third-party indexer in the trust path, P5/P13).
pub(crate) const ITEM_LISTED_TOPIC0: &str =
    "0x90aecdd7f5269ac7f11bea516b4768d0391e0a54aabc19aea64c7758104f66d2";
pub(crate) const ITEM_SOLD_TOPIC0: &str =
    "0x60cd9eee664e26e142eb54813d426c273cd85605b8bfb72f707e4f2927b6a955";
pub(crate) const ITEM_UNLISTED_TOPIC0: &str =
    "0xdb6bedce61ad043a5e9d9ac95f248702233e64e5818e58734aa38e7fd86db415";

/// The 32-byte topic form of an address (left-padded), for an `eth_getLogs` indexed-topic filter.
pub(crate) fn address_topic(addr: &str) -> Option<String> {
    address_word(addr).map(|w| format!("0x{w}"))
}

/// A decoded marketplace trade event for the Activity/History view. `price` is raw minor units
/// (pricePerToken for a listing; unitPrice for a sale) — the gateway formats it with the pay-token decimals.
#[derive(Debug, serde::Serialize)]
pub(crate) struct TradeEvent {
    pub kind: &'static str, // "list" | "sale" | "unlist"
    pub seller: Option<String>,
    pub buyer: Option<String>,
    pub price: Option<String>,
    pub pay_token: Option<String>,
    pub quantity: Option<String>,
    pub block: Option<u64>,
    pub tx: Option<String>,
}

/// Extract the low-20-byte address from a 32-byte ABI word/topic (with or without `0x`). Lossy by design
/// (display only); returns a lowercased `0x…` address.
fn word_to_address_lossy(word: &str) -> String {
    let h = word
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    let tail = if h.len() >= 40 { &h[h.len() - 40..] } else { h };
    format!("0x{}", tail.to_lowercase())
}

/// Decode one `eth_getLogs` entry into a `TradeEvent` by its topic0. Returns `None` for unrelated logs or
/// any malformed/short record (fail-soft — a hostile log never panics or fabricates a row).
pub(crate) fn decode_trade_log(log: &serde_json::Value) -> Option<TradeEvent> {
    let topics = log.get("topics").and_then(serde_json::Value::as_array)?;
    let topic0 = topics.first().and_then(serde_json::Value::as_str)?;
    let topic = |i: usize| topics.get(i).and_then(serde_json::Value::as_str);
    let data_clean = log
        .get("data")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .trim_start_matches("0x");
    let dword = |i: usize| data_clean.get(i * 64..i * 64 + 64);
    let u128_str = |w: &str| word_to_u128(w).ok().map(|n| n.to_string());
    let block = log
        .get("blockNumber")
        .and_then(serde_json::Value::as_str)
        .and_then(|s| u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok());
    let tx = log
        .get("transactionHash")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);

    if topic0.eq_ignore_ascii_case(ITEM_LISTED_TOPIC0) {
        // ItemListed(seller^, op^, tkId^, quantity, pricePerToken, payToken)
        Some(TradeEvent {
            kind: "list",
            seller: topic(1).map(word_to_address_lossy),
            buyer: None,
            quantity: dword(0).and_then(u128_str),
            price: dword(1).and_then(u128_str),
            pay_token: dword(2).map(word_to_address_lossy),
            block,
            tx,
        })
    } else if topic0.eq_ignore_ascii_case(ITEM_SOLD_TOPIC0) {
        // ItemSold(seller, buyer^, op^, tkId^, payToken, unitPrice, price) — seller is in data word 0.
        Some(TradeEvent {
            kind: "sale",
            seller: dword(0).map(word_to_address_lossy),
            buyer: topic(1).map(word_to_address_lossy),
            quantity: None,
            price: dword(2).and_then(u128_str),
            pay_token: dword(1).map(word_to_address_lossy),
            block,
            tx,
        })
    } else if topic0.eq_ignore_ascii_case(ITEM_UNLISTED_TOPIC0) {
        // ItemUnlisted(seller^, op^, tkId^, quantity)
        Some(TradeEvent {
            kind: "unlist",
            seller: topic(1).map(word_to_address_lossy),
            buyer: None,
            quantity: dword(0).and_then(u128_str),
            price: None,
            pay_token: None,
            block,
            tx,
        })
    } else {
        None
    }
}

/// Decode `(address,uint256)[]` from a `royaltyInfo` return and convert each `(receiver, amount)` into
/// `(receiver, pct)` where `pct = amount / sale_base * 100`. Fail-soft (Err, never panic) on any
/// malformed/hostile offset/length/word; the party count is bounded before allocation.
pub(crate) fn decode_royalty_distributions(
    result: &str,
    sale_base: u128,
) -> Result<Vec<(String, f64)>, String> {
    const MAX_PARTIES: usize = 64;
    if sale_base == 0 {
        return Err("royaltyInfo sale base must be non-zero".to_string());
    }
    let clean = result.trim().trim_start_matches("0x");
    if !clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("royaltyInfo return is not hex".to_string());
    }
    // word 0 = byte-offset to the array; word at that offset = length; then `length` (address, amount) pairs.
    let off = word_to_usize(
        clean
            .get(0..64)
            .ok_or("royaltyInfo return too short for offset")?,
    )?;
    let len_at = off.checked_mul(2).ok_or("royaltyInfo offset overflow")?;
    let len_end = len_at
        .checked_add(64)
        .ok_or("royaltyInfo length overflow")?;
    let len = word_to_usize(
        clean
            .get(len_at..len_end)
            .ok_or("royaltyInfo return too short for length")?,
    )?;
    if len > MAX_PARTIES {
        return Err(format!(
            "royaltyInfo length {len} exceeds bound {MAX_PARTIES}"
        ));
    }
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        // Each tuple is two static words: [address][amount].
        let base = len_end
            .checked_add(i.checked_mul(128).ok_or("royaltyInfo index overflow")?)
            .ok_or("royaltyInfo index overflow")?;
        let addr_end = base.checked_add(64).ok_or("royaltyInfo word overflow")?;
        let amt_end = addr_end
            .checked_add(64)
            .ok_or("royaltyInfo word overflow")?;
        let addr_word = clean
            .get(base..addr_end)
            .ok_or("royaltyInfo return too short for address")?;
        let amt_word = clean
            .get(addr_end..amt_end)
            .ok_or("royaltyInfo return too short for amount")?;
        if addr_word[..24].bytes().any(|b| b != b'0') {
            return Err("royaltyInfo address word has non-zero high bytes".to_string());
        }
        let amount = word_to_u128(amt_word)?;
        out.push((
            format!("0x{}", &addr_word[24..]),
            (amount as f64) / (sale_base as f64) * 100.0,
        ));
    }
    Ok(out)
}

/// Live `royaltyInfo(SALE_BASE)` on the asset's Operative -> the per-receiver royalty distribution as
/// `(address, pct)`. READ-ONLY. Fail-closed: a revert (FREE / non-royalty operative) propagates as Err so
/// the caller hides the splits panel.
pub(crate) fn royalty_info_live(operative: &str) -> Result<Vec<(String, f64)>, String> {
    let data = format!("0x{ROYALTY_INFO_SELECTOR}{ROYALTY_SALE_BASE:064x}");
    let result = super::chain_tx::contract_call_live(operative, &data)?;
    decode_royalty_distributions(&result, ROYALTY_SALE_BASE)
}

/// Live `resellerCut()` on the asset's Operative -> the secondary-sale royalty as a human percentage
/// (`raw/10`). READ-ONLY. Fail-closed: reverts on a non-resellable (BUY_ONCE/FREE) operative.
pub(crate) fn reseller_cut_live(operative: &str) -> Result<f64, String> {
    let data = format!("0x{RESELLER_CUT_SELECTOR}");
    let result = super::chain_tx::contract_call_live(operative, &data)?;
    let word = result
        .trim()
        .trim_start_matches("0x")
        .get(0..64)
        .ok_or("resellerCut return too short")?
        .to_string();
    Ok(share_per_1000_to_pct(word_to_u128(&word)?))
}

/// Live `protocolShares()` on CoreStorage -> the protocol (Elacity) cut as a human percentage
/// (`shares/10`). READ-ONLY. Fail-closed on a malformed return.
pub(crate) fn protocol_shares_live(core_storage: &str) -> Result<f64, String> {
    let data = format!("0x{PROTOCOL_SHARES_SELECTOR}");
    let result = super::chain_tx::contract_call_live(core_storage, &data)?;
    let word = result
        .trim()
        .trim_start_matches("0x")
        .get(0..64)
        .ok_or("protocolShares return too short")?
        .to_string();
    Ok(share_per_1000_to_pct(word_to_u128(&word)?))
}

// ---- Multicall3 batching: collapse per-card discovery reads into 2 `eth_call`s for the whole page ----
// Same reads, same decoders as the per-card path (P10 — one canonical read); only the TRANSPORT is
// batched. Still a READ-ONLY `eth_call` through the sole RPC declarant (chain-provider via `chain_tx`),
// declaring no funds and holding no keys (P3). `allowFailure` is always true so one reverting sub-call
// degrades a single card (sellers_ok=false / dropped listing), never the whole page (P11).

/// Canonical Multicall3 — deployed at this same address on every supported chain (incl. Base),
/// CONTRACTS.md §3.6. `aggregate3` is a `view`; this is a read aggregator, not a fund mover.
const MULTICALL3: &str = "0xca11bde05977b3631167028862be2a173976ca11";
/// `aggregate3((address,bool,bytes)[]) returns ((bool,bytes)[])` — keccak256 selector, first 4 bytes.
const AGGREGATE3_SELECTOR: &str = "82ad56cb";

/// ABI-encode ONE `Call3` tuple `(address target, bool allowFailure=true, bytes callData)`. Fail-closed
/// on a bad target/odd calldata so a malformed read can't straddle the batch.
fn encode_call3(target: &str, calldata: &str) -> Result<String, String> {
    let addr =
        address_word(target).ok_or_else(|| format!("multicall target not an address: {target}"))?;
    let data = calldata.trim().trim_start_matches("0x").to_lowercase();
    if data.len() % 2 != 0 || !data.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("multicall calldata is not even-length hex".to_string());
    }
    let allow = format!("{:064x}", 1u8); // allowFailure = true
    let bytes_off = format!("{:064x}", 96u64); // bytes start 3 words into the tuple
    let len = format!("{:064x}", data.len() / 2);
    let mut padded = data;
    let rem = padded.len() % 64;
    if rem != 0 {
        padded.push_str(&"0".repeat(64 - rem));
    }
    Ok(format!("{addr}{allow}{bytes_off}{len}{padded}"))
}

/// ABI-encode `aggregate3(Call3[])` calldata. `calls` = `(target, calldata)`; `allowFailure` is always
/// true. Fail-closed: a bad sub-call aborts the encode (the caller falls back to per-read).
pub(crate) fn encode_aggregate3(calls: &[(String, String)]) -> Result<String, String> {
    let tuples: Vec<String> = calls
        .iter()
        .map(|(t, d)| encode_call3(t, d))
        .collect::<Result<_, _>>()?;
    // Dynamic array of dynamic tuples: one offset word per element, offsets relative to the first.
    let mut offsets = String::new();
    let mut tails = String::new();
    let mut cursor = (tuples.len() * 32) as u64;
    for tpl in &tuples {
        offsets.push_str(&format!("{cursor:064x}"));
        cursor += (tpl.len() / 2) as u64;
        tails.push_str(tpl);
    }
    let array_off = format!("{:064x}", 32u64); // word0 -> array begins at byte 32
    let array_len = format!("{:064x}", tuples.len());
    Ok(format!(
        "0x{AGGREGATE3_SELECTOR}{array_off}{array_len}{offsets}{tails}"
    ))
}

/// Decode `aggregate3`'s `Result[]` of `(bool success, bytes returnData)`. Each entry is
/// `(success, "0x"+hex returnData)` (empty data on a reverted sub-call). Fail-soft on any malformed
/// offset/length — a hostile RPC word can be huge, so every index is bounds-checked.
pub(crate) fn decode_aggregate3(result: &str) -> Result<Vec<(bool, String)>, String> {
    const MAX_RESULTS: usize = 1024;
    let clean = result.trim().trim_start_matches("0x");
    if !clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("aggregate3 return is not hex".to_string());
    }
    let word = |at: usize| -> Result<&str, String> {
        clean
            .get(at..at + 64)
            .ok_or_else(|| "aggregate3 return too short".to_string())
    };
    let arr_off = word_to_usize(word(0)?)?
        .checked_mul(2)
        .ok_or("aggregate3 offset overflow")?;
    let len = word_to_usize(word(arr_off)?)?;
    if len > MAX_RESULTS {
        return Err(format!(
            "aggregate3 length {len} exceeds bound {MAX_RESULTS}"
        ));
    }
    let region = arr_off
        .checked_add(64)
        .ok_or("aggregate3 region overflow")?;
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let off_at = region
            .checked_add(i.checked_mul(64).ok_or("aggregate3 index overflow")?)
            .ok_or("aggregate3 index overflow")?;
        let elem = region
            .checked_add(
                word_to_usize(word(off_at)?)?
                    .checked_mul(2)
                    .ok_or("aggregate3 elem overflow")?,
            )
            .ok_or("aggregate3 elem overflow")?;
        let success = word_to_usize(word(elem)?)? != 0;
        let bytes_at = elem
            .checked_add(
                word_to_usize(word(
                    elem.checked_add(64).ok_or("aggregate3 bytes overflow")?,
                )?)?
                .checked_mul(2)
                .ok_or("aggregate3 bytes overflow")?,
            )
            .ok_or("aggregate3 bytes overflow")?;
        let blen = word_to_usize(word(bytes_at)?)?
            .checked_mul(2)
            .ok_or("aggregate3 blen overflow")?;
        let data_at = bytes_at.checked_add(64).ok_or("aggregate3 data overflow")?;
        let data = clean
            .get(
                data_at
                    ..data_at
                        .checked_add(blen)
                        .ok_or("aggregate3 data overflow")?,
            )
            .ok_or("aggregate3 return data out of bounds")?;
        out.push((success, format!("0x{data}")));
    }
    Ok(out)
}

/// The compact per-asset listing facts a Discover/Explore CARD needs, read for a whole page in TWO
/// batched `aggregate3` round-trips (was N+ `eth_call`s per card).
pub(crate) struct CardBrief {
    /// `sellersOf` read succeeded (distinguishes "no sellers" from "couldn't read" — fail-closed display).
    pub sellers_ok: bool,
    pub resale_pct: Option<f64>,
    /// Active listings only (supply > 0): `(terms, supply)`.
    pub listings: Vec<(super::buy_authority::BoundTerms, u128)>,
}

/// Decode a single-word `resellerCut()` return (revert/short -> `None`).
fn decode_reseller_cut_word(ret: &str) -> Option<f64> {
    let w = ret.trim().trim_start_matches("0x").get(0..64)?;
    word_to_u128(w).ok().map(share_per_1000_to_pct)
}

/// Batched discovery enrichment for many cards via Multicall3 — two `eth_call`s total:
///   round 1, per card: `sellersOf(operative)` + `resellerCut(operative)`
///   round 2, per (card, seller): `listings(operative, seller)`
/// Returns one `CardBrief` per input item, index-aligned. Fail-closed: `Err` if the batch `eth_call`
/// itself fails (caller falls back to the per-card path); per-sub-call reverts surface as
/// `sellers_ok=false` / dropped listings — never a fabricated price. `items` = `(operative, token_id_word)`.
pub(crate) fn listing_briefs_batched(
    gateway: &str,
    items: &[(String, String)],
) -> Result<Vec<CardBrief>, String> {
    const MAX_SELLERS_PER_CARD: usize = 8;
    if items.is_empty() {
        return Ok(Vec::new());
    }
    // ---- round 1: sellersOf + resellerCut per card ----
    let mut calls1: Vec<(String, String)> = Vec::with_capacity(items.len() * 2);
    for (operative, _word) in items {
        let sellers_cd = encode_sellers_of(operative, super::buy_authority::ACCESS_TOKEN_ID_WORD)?;
        calls1.push((gateway.to_string(), sellers_cd));
        calls1.push((operative.clone(), format!("0x{RESELLER_CUT_SELECTOR}")));
    }
    let ret1 = super::chain_tx::contract_call_live(MULTICALL3, &encode_aggregate3(&calls1)?)?;
    let res1 = decode_aggregate3(&ret1)?;
    if res1.len() != calls1.len() {
        return Err("aggregate3 round-1 result count mismatch".to_string());
    }
    struct Pending {
        sellers_ok: bool,
        resale_pct: Option<f64>,
    }
    let mut pending: Vec<Pending> = Vec::with_capacity(items.len());
    let mut seller_jobs: Vec<(usize, String)> = Vec::new(); // (card_idx, seller)
    for i in 0..items.len() {
        let (s_ok, s_ret) = &res1[i * 2];
        let (_r_ok, r_ret) = &res1[i * 2 + 1];
        let (sellers_ok, sellers) = if *s_ok {
            match decode_address_array(s_ret) {
                Ok(mut v) => {
                    v.truncate(MAX_SELLERS_PER_CARD);
                    (true, v)
                }
                Err(_) => (false, Vec::new()),
            }
        } else {
            (false, Vec::new())
        };
        for seller in sellers {
            seller_jobs.push((i, seller));
        }
        pending.push(Pending {
            sellers_ok,
            resale_pct: decode_reseller_cut_word(r_ret),
        });
    }
    // ---- round 2: listings(operative, seller) per (card, seller) ----
    let mut listings_by_card: Vec<Vec<(super::buy_authority::BoundTerms, u128)>> =
        (0..items.len()).map(|_| Vec::new()).collect();
    if !seller_jobs.is_empty() {
        let mut calls2: Vec<(String, String)> = Vec::with_capacity(seller_jobs.len());
        for (card_idx, seller) in &seller_jobs {
            let (operative, _word) = &items[*card_idx];
            calls2.push((
                gateway.to_string(),
                super::buy_authority::encode_listings(operative, seller),
            ));
        }
        let ret2 = super::chain_tx::contract_call_live(MULTICALL3, &encode_aggregate3(&calls2)?)?;
        let res2 = decode_aggregate3(&ret2)?;
        if res2.len() != calls2.len() {
            return Err("aggregate3 round-2 result count mismatch".to_string());
        }
        for (j, (card_idx, seller)) in seller_jobs.iter().enumerate() {
            let (_op, word) = &items[*card_idx];
            let (ok, ret) = &res2[j];
            if !*ok {
                continue;
            }
            if let Ok((terms, supply)) =
                super::buy_authority::decode_listing_return(ret, seller, word)
            {
                if supply > 0 {
                    listings_by_card[*card_idx].push((terms, supply));
                }
            }
        }
    }
    let mut out = Vec::with_capacity(items.len());
    for (i, p) in pending.into_iter().enumerate() {
        out.push(CardBrief {
            sellers_ok: p.sellers_ok,
            resale_pct: p.resale_pct,
            listings: std::mem::take(&mut listings_by_card[i]),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_access_batched_empty_is_no_network_empty() {
        // Zero KIDs must short-circuit to an empty result with no RPC (the Vault sweep for a wallet that
        // matched nothing in the discovery window): never an error, never a fabricated holding.
        let out = has_access_batched(
            "0x00000000000000000000000000000000000000aa",
            "0x00000000000000000000000000000000000000bb",
            &[],
        )
        .expect("empty batch is Ok");
        assert!(out.is_empty());
    }

    #[test]
    fn my_listings_batched_empty_is_no_network_empty() {
        // No operatives -> empty result, no RPC (a wallet with nothing in the discovery window).
        let out = my_listings_batched("0x00000000000000000000000000000000000000aa", &[])
            .expect("empty batch is Ok");
        assert!(out.is_empty());
    }

    #[test]
    fn aggregate3_round_trips_through_encode_and_decode() {
        // Encode a 2-call batch, then feed an aggregate3-shaped RESPONSE built from those same
        // calldatas back through the decoder — exercising the dynamic array/tuple/bytes offsets.
        let calls = vec![
            (
                "0x00000000000000000000000000000000000000aa".to_string(),
                "0xdeadbeef".to_string(),
            ),
            (
                "0x00000000000000000000000000000000000000bb".to_string(),
                "0x".to_string(),
            ),
        ];
        let encoded = encode_aggregate3(&calls).expect("encode");
        assert!(encoded.starts_with(&format!("0x{AGGREGATE3_SELECTOR}")));

        // Hand-build a Result[]{(true,0x1234),(false,0x)} response and decode it.
        let w = |n: u64| format!("{n:064x}");
        let data_word = format!("{:0<64}", "1234"); // 2-byte payload, right-padded to a word
        let e0 = [w(1), w(64), w(2), data_word].concat(); // success, bytes-off, len=2, data (4 words)
        let e1 = [w(0), w(64), w(0)].concat(); // success=0, bytes-off, len=0 (3 words)
        let body = [
            w(32),  // word0 -> array at byte 32
            w(2),   // length = 2
            w(64),  // elem0 offset (after 2 offset words)
            w(192), // elem1 offset (elem0 is 128 bytes)
            e0,
            e1,
        ]
        .concat();
        let decoded = decode_aggregate3(&format!("0x{body}")).expect("decode");
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0], (true, "0x1234".to_string()));
        assert_eq!(decoded[1], (false, "0x".to_string()));
    }

    #[test]
    fn decode_aggregate3_fails_closed_on_short_return() {
        assert!(decode_aggregate3("0x1234").is_err());
        assert!(decode_aggregate3("0xnothex").is_err());
    }

    #[test]
    fn decode_trade_log_reads_listed_and_sold_abi_layouts() {
        let addr = |b: u8| format!("0x{}", format!("{b:02x}").repeat(20));
        let topic_addr = |b: u8| format!("0x{:0>64}", format!("{b:02x}").repeat(20));
        let word_u = |n: u128| format!("{n:064x}");
        let word_addr = |b: u8| format!("{:0>64}", format!("{b:02x}").repeat(20));

        // ItemListed(seller^, op^, tkId^, quantity, pricePerToken, payToken)
        let listed = serde_json::json!({
            "topics": [ITEM_LISTED_TOPIC0, topic_addr(0x11), topic_addr(0x22), word_u(1)],
            "data": format!("0x{}{}{}", word_u(10_000), word_u(20_000), word_addr(0x33)),
            "blockNumber": "0x10",
            "transactionHash": "0xabc",
        });
        let ev = decode_trade_log(&listed).expect("listed decodes");
        assert_eq!(ev.kind, "list");
        assert_eq!(ev.seller.as_deref(), Some(addr(0x11).as_str()));
        assert_eq!(ev.quantity.as_deref(), Some("10000"));
        assert_eq!(ev.price.as_deref(), Some("20000"));
        assert_eq!(ev.pay_token.as_deref(), Some(addr(0x33).as_str()));
        assert_eq!(ev.block, Some(16));

        // ItemSold(seller, buyer^, op^, tkId^, payToken, unitPrice, price)
        let sold = serde_json::json!({
            "topics": [ITEM_SOLD_TOPIC0, topic_addr(0xaa), topic_addr(0x22), word_u(1)],
            "data": format!("0x{}{}{}{}", word_addr(0xbb), word_addr(0x33), word_u(20_000), word_u(40_000)),
            "blockNumber": "0x20",
            "transactionHash": "0xdef",
        });
        let ev = decode_trade_log(&sold).expect("sold decodes");
        assert_eq!(ev.kind, "sale");
        assert_eq!(ev.buyer.as_deref(), Some(addr(0xaa).as_str()));
        assert_eq!(ev.seller.as_deref(), Some(addr(0xbb).as_str()));
        assert_eq!(ev.pay_token.as_deref(), Some(addr(0x33).as_str()));
        assert_eq!(ev.price.as_deref(), Some("20000")); // unitPrice (per-token), not the line total
        assert_eq!(ev.block, Some(32));

        // Unrelated topic0 → None (fail-soft, never a fabricated row).
        let other = serde_json::json!({ "topics": [format!("0x{:064x}", 0)], "data": "0x" });
        assert!(decode_trade_log(&other).is_none());
    }

    #[test]
    fn extract_cid_matches_content_market_rules() {
        assert_eq!(
            extract_cid("ipfs://bafyXYZ/metadata.json").as_deref(),
            Some("bafyXYZ")
        );
        assert_eq!(
            extract_cid("https://gw/ipfs/QmAbc/metadata.json").as_deref(),
            Some("QmAbc")
        );
        assert_eq!(extract_cid("QmAbc/metadata.json").as_deref(), Some("QmAbc"));
        assert_eq!(extract_cid("bafyOnly").as_deref(), Some("bafyOnly"));
        assert_eq!(extract_cid("https://example.com/x.json"), None); // not an ipfs uri
        assert_eq!(extract_cid("ipfs://"), None);
    }

    #[test]
    fn extract_cid_subpath_isolates_the_in_dir_path() {
        // The UnixFS-directory shape (`ipfs://<dirCid>/metadata.json`) — the subpath the gateway must fetch.
        assert_eq!(
            extract_cid_subpath("ipfs://bafyDir/metadata.json").as_deref(),
            Some("metadata.json")
        );
        assert_eq!(
            extract_cid_subpath("https://gw/ipfs/QmDir/metadata.json").as_deref(),
            Some("metadata.json")
        );
        assert_eq!(
            extract_cid_subpath("QmDir/metadata.json").as_deref(),
            Some("metadata.json")
        );
        // Nested paths keep their full in-dir path.
        assert_eq!(
            extract_cid_subpath("ipfs://bafyDir/meta/asset.json").as_deref(),
            Some("meta/asset.json")
        );
        // Bare single-file CIDs have no subpath -> None (the bare CID resolves to the file directly).
        assert_eq!(extract_cid_subpath("ipfs://bafyOnly"), None);
        assert_eq!(extract_cid_subpath("QmAbc"), None);
        // A trailing slash with no file is not a subpath.
        assert_eq!(extract_cid_subpath("ipfs://bafyDir/"), None);
        assert_eq!(extract_cid_subpath("https://example.com/x.json"), None); // not an ipfs uri
    }

    #[test]
    fn parse_dash_preview_reads_bento4_clear_manifest() {
        // Trimmed from a real preview stream.mpd (clear AV1 video + AAC audio, SegmentTimeline).
        let mpd = r#"<?xml version="1.0" ?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period>
    <AdaptationSet mimeType="video/mp4" segmentAlignment="true">
      <SegmentTemplate timescale="12800" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/seg-$Number$.m4s" startNumber="1">
        <SegmentTimeline><S d="128000"/><S d="63488"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="video/av01/1" codecs="av01.0.00M.10" width="1920" height="1080" bandwidth="277525"/>
      <Representation id="video/av01/3" codecs="av01.0.00M.10" width="854" height="480" bandwidth="109606"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4" lang="en">
      <SegmentTemplate timescale="48000" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/seg-$Number$.m4s" startNumber="1">
        <SegmentTimeline><S d="480256"/><S d="237568"/><S d="5120"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="audio/en/mp4a.40.2" codecs="mp4a.40.2" bandwidth="141680"/>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let tracks = parse_dash_preview(mpd);
        assert_eq!(tracks.len(), 2, "one video + one audio track");
        let v = &tracks[0];
        assert_eq!(v.kind, "video");
        // Lowest-bandwidth video representation is chosen (480p over 1080p).
        assert_eq!(v.mime, "video/mp4; codecs=\"av01.0.00M.10\"");
        assert_eq!(v.init_path, "video/av01/3/init.mp4");
        assert_eq!(
            v.seg_paths,
            vec!["video/av01/3/seg-1.m4s", "video/av01/3/seg-2.m4s"]
        );
        let a = &tracks[1];
        assert_eq!(a.kind, "audio");
        assert_eq!(a.mime, "audio/mp4; codecs=\"mp4a.40.2\"");
        assert_eq!(a.init_path, "audio/en/mp4a.40.2/init.mp4");
        assert_eq!(a.seg_paths.len(), 3, "three audio segments from timeline");
        // Garbage degrades to empty (fail-soft), never panics.
        assert!(parse_dash_preview("not xml").is_empty());
    }

    #[test]
    fn parse_asset_metadata_reads_the_content_market_paths() {
        let meta = serde_json::json!({
            "name": "Aerials — Episode 1",
            "description": "A short aerial film.",
            "image": "ipfs://QmPoster/poster.png",
            "kid": "0x9C2A000000000000000000000000E1A1",
            "media": { "uri": "ipfs://QmContent/enc.bin", "contentType": "video/mp4" }
        });
        let a = parse_asset_metadata(&meta);
        assert_eq!(a.name.as_deref(), Some("Aerials — Episode 1"));
        assert_eq!(a.image_url.as_deref(), Some("ipfs://QmPoster/poster.png"));
        assert_eq!(a.content_cid.as_deref(), Some("QmContent")); // media.uri -> extract_cid
        assert_eq!(a.mime_type.as_deref(), Some("video/mp4"));
        // kid (used to bind the acquire CID to the gated KID); normalize_kid is case/0x-insensitive
        assert_eq!(
            normalize_kid(a.kid.as_deref().unwrap()),
            normalize_kid("9c2a000000000000000000000000e1a1")
        );
        assert!(normalize_kid("0xdeadbeef").is_none()); // not 32 hex -> None (fail closed)
        assert!(normalize_kid("9c2a000000000000000000000000e1a1").is_some());
        // poster fallback to media.previewURL when image is absent; empty strings -> None
        let meta2 =
            serde_json::json!({ "name": "", "media": { "previewURL": "ipfs://QmPrev/p.png" } });
        let b = parse_asset_metadata(&meta2);
        assert!(b.name.is_none());
        assert_eq!(b.image_url.as_deref(), Some("ipfs://QmPrev/p.png"));
        assert!(b.content_cid.is_none());
    }

    #[test]
    fn encode_has_access_pins_selector_and_left_aligns_the_kid() {
        let holder = "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D";
        let kid = "0x9c2a000000000000000000000000e1a1";
        let data = encode_has_access(holder, kid).unwrap();
        assert!(data.starts_with("0x54d42821"));
        assert_eq!(data.len(), 2 + 8 + 64 + 64); // 0x + selector + holder word + kid word
                                                 // bytes16 left-aligned: the KID hex leads the last word, then zero padding to 64.
        assert!(data
            .to_lowercase()
            .ends_with(&format!("{:0<64}", "9c2a000000000000000000000000e1a1")));
        assert!(encode_has_access("0xnothex", kid).is_err());
        assert!(encode_has_access(holder, "0xtooshort").is_err());
    }

    #[test]
    fn decode_bool_reads_true_false_and_fails_soft() {
        assert!(decode_bool(&format!("0x{:064x}", 1)));
        assert!(!decode_bool(&format!("0x{:064x}", 0)));
        assert!(!decode_bool("0x")); // empty -> false
        assert!(!decode_bool("0xzz")); // non-hex -> false (fail soft)
    }

    #[test]
    fn encode_sellers_of_pins_selector_and_args() {
        let op = "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D";
        let tid = format!("{:064x}", 7u128);
        let data = encode_sellers_of(op, &tid).unwrap();
        assert!(data.starts_with("0x997eab2d"));
        assert!(data.to_lowercase().ends_with(&tid)); // tokenId is the trailing word
        assert!(data.to_lowercase().contains(&op[2..].to_lowercase())); // operative left-padded in
        assert_eq!(data.len(), 2 + 8 + 64 + 64); // 0x + selector + 2 words
                                                 // fail closed on a bad address / non-word tokenId
        assert!(encode_sellers_of("0xnothex", &tid).is_err());
        assert!(encode_sellers_of(op, "0x07").is_err());
    }

    #[test]
    fn share_per_1000_to_pct_matches_elacity_scale() {
        // elacity-web canonical: resellerCut/protocolShares are 1000-scale -> %. 150 -> 15%, 100 -> 10%.
        assert_eq!(share_per_1000_to_pct(150), 15.0);
        assert_eq!(share_per_1000_to_pct(100), 10.0);
        assert_eq!(share_per_1000_to_pct(0), 0.0);
        assert_eq!(share_per_1000_to_pct(900), 90.0); // 1000-scale: 900 units == 90%
    }

    #[test]
    fn decode_royalty_distributions_reads_pairs_and_fails_soft() {
        // royaltyInfo(1e12) -> [(addrA, 550e9), (addrB, 200e9)] => 55% / 20%.
        let base = 1_000_000_000_000u128;
        let a = "1111111111111111111111111111111111111111";
        let b = "2222222222222222222222222222222222222222";
        let off = format!("{:064x}", 32);
        let len = format!("{:064x}", 2);
        let aw = format!("{a:0>64}");
        let bw = format!("{b:0>64}");
        let amt_a = format!("{:064x}", 550_000_000_000u128); // 55% of 1e12
        let amt_b = format!("{:064x}", 200_000_000_000u128); // 20% of 1e12
        let ret = format!("0x{off}{len}{aw}{amt_a}{bw}{amt_b}");
        let dist = decode_royalty_distributions(&ret, base).unwrap();
        assert_eq!(dist.len(), 2);
        assert_eq!(dist[0].0, format!("0x{a}"));
        assert!((dist[0].1 - 55.0).abs() < 1e-9);
        assert_eq!(dist[1].0, format!("0x{b}"));
        assert!((dist[1].1 - 20.0).abs() < 1e-9);
        // empty distribution
        let len0 = format!("{:064x}", 0);
        assert!(
            decode_royalty_distributions(&format!("0x{off}{len0}"), base)
                .unwrap()
                .is_empty()
        );
        // hostile huge length -> fail closed (no panic, no huge alloc)
        let huge = format!("{:064x}", u64::MAX);
        assert!(decode_royalty_distributions(&format!("0x{off}{huge}"), base).is_err());
        // non-hex / truncated / empty return -> Err
        assert!(decode_royalty_distributions("0xzz", base).is_err());
        assert!(decode_royalty_distributions("0x", base).is_err());
        // a non-address-clean high half (dirty word) -> fail closed
        let dirty = format!("{:0>64}", "ff".to_string() + a); // non-zero high bytes
        assert!(decode_royalty_distributions(
            &format!("0x{off}{len}{dirty}{amt_a}{bw}{amt_b}"),
            base
        )
        .is_err());
        // zero sale base -> Err (no divide-by-zero)
        assert!(decode_royalty_distributions(&ret, 0).is_err());
    }

    #[test]
    fn word_to_u128_reads_and_fails_closed() {
        assert_eq!(
            word_to_u128(&format!("{:064x}", 1_000_000u128)).unwrap(),
            1_000_000
        );
        assert!(word_to_u128(&"f".repeat(64)).is_err()); // beyond u128 (high half set)
        assert!(word_to_u128("0x").is_err()); // wrong length
        assert!(word_to_u128(&"z".repeat(64)).is_err()); // non-hex
    }

    #[test]
    fn decode_address_array_reads_sellers_and_fails_soft() {
        // [offset=0x20][len=2][addrA][addrB]
        let a = "1111111111111111111111111111111111111111";
        let b = "2222222222222222222222222222222222222222";
        let off = format!("{:064x}", 32);
        let len = format!("{:064x}", 2);
        let aw = format!("{a:0>64}");
        let bw = format!("{b:0>64}");
        let ret = format!("0x{off}{len}{aw}{bw}");
        let sellers = decode_address_array(&ret).unwrap();
        assert_eq!(sellers, vec![format!("0x{a}"), format!("0x{b}")]);
        // empty array
        let len0 = format!("{:064x}", 0);
        let empty = format!("0x{off}{len0}");
        assert!(decode_address_array(&empty).unwrap().is_empty());
        // hostile: a forged huge length must fail closed (no panic, no huge alloc)
        let huge_len = format!("{:064x}", u64::MAX);
        let huge = format!("0x{off}{huge_len}");
        assert!(decode_address_array(&huge).is_err());
        // non-hex / truncated -> Err
        assert!(decode_address_array("0xzz").is_err());
        assert!(decode_address_array("0x20").is_err());
    }
}
