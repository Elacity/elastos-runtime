use super::*;

pub(super) fn rights_method<'a>(
    network: &'a ChainNetwork,
    id: &str,
    contract: &str,
) -> Result<&'a RightsMethod, Response> {
    let configured = network
        .rights_methods
        .iter()
        .find(|method| method.id == id)
        .ok_or_else(|| {
            Response::error(
                "rights_query_not_configured",
                &format!("typed {id} ABI is not configured for {}", network.id),
            )
        })?;
    if !configured.contract.eq_ignore_ascii_case(contract) {
        return Err(Response::error(
            "rights_contract_not_allowed",
            "requested rights contract is not configured for this network",
        ));
    }
    Ok(configured)
}

/// Real Base ABI: `hasAccessByContentId(address holder, bytes16 contentId) -> bool`
/// (selector `0x54d42821`, confirmed against `~/.pc2` `contracts/abis.ts`). Two static
/// words: the holder address, then the `bytes16` contentId (KID) left-aligned. PURE: no
/// RPC, no keys. `right` is NOT an on-chain parameter here — access is binary per
/// contentId — so the gateway keeps `right` only in the signed decision receipt.
pub(super) fn encode_has_access_by_content_id_address_bytes16(
    selector: &str,
    subject: &str,
    content_id: &str,
) -> Result<String, String> {
    let mut bytes = decode_hex(selector, Some(4), "EVM function selector")?;
    bytes.extend_from_slice(&abi_word_address(subject)?);
    bytes.extend_from_slice(&abi_word_bytes16(content_id)?);
    Ok(format!("0x{}", encode_hex(&bytes)))
}

/// Legacy/guessed `hasAccessByContentId(string,address,string)` shape — kept because the
/// typed ABI is config-selectable, but NOT the real Base ABI. Prefer
/// `encode_has_access_by_content_id_address_bytes16`.
pub(super) fn encode_has_access_by_content_id_call(
    selector: &str,
    content_id: &str,
    subject: &str,
    right: &str,
) -> Result<String, String> {
    let mut bytes = decode_hex(selector, Some(4), "EVM function selector")?;
    let content = abi_encode_string(content_id.as_bytes());
    let right = abi_encode_string(right.as_bytes());
    let content_offset = 32 * 3;
    let right_offset = content_offset + content.len();

    bytes.extend_from_slice(&abi_word_usize(content_offset));
    bytes.extend_from_slice(&abi_word_address(subject)?);
    bytes.extend_from_slice(&abi_word_usize(right_offset));
    bytes.extend_from_slice(&content);
    bytes.extend_from_slice(&right);

    Ok(format!("0x{}", encode_hex(&bytes)))
}

pub(super) fn encode_erc1271_is_valid_signature_call(
    message_hash: &[u8],
    signature: &[u8],
) -> String {
    let mut bytes = vec![0x16, 0x26, 0xba, 0x7e];
    bytes.extend_from_slice(message_hash);
    bytes.extend_from_slice(&abi_word_usize(64));
    bytes.extend_from_slice(&abi_encode_bytes(signature));
    format!("0x{}", encode_hex(&bytes))
}

pub(super) fn abi_encode_bytes(value: &[u8]) -> Vec<u8> {
    let mut encoded = abi_word_usize(value.len());
    encoded.extend_from_slice(value);
    let padding = (32 - (value.len() % 32)) % 32;
    encoded.extend(std::iter::repeat_n(0, padding));
    encoded
}

pub(super) fn abi_encode_string(value: &[u8]) -> Vec<u8> {
    let mut encoded = abi_word_usize(value.len());
    encoded.extend_from_slice(value);
    let padding = (32 - (value.len() % 32)) % 32;
    encoded.extend(std::iter::repeat_n(0, padding));
    encoded
}

pub(super) fn abi_word_usize(value: usize) -> Vec<u8> {
    let mut word = vec![0u8; 32];
    word[24..32].copy_from_slice(&(value as u64).to_be_bytes());
    word
}

pub(super) fn abi_word_address(address: &str) -> Result<Vec<u8>, String> {
    let address = decode_hex(address, Some(20), "EVM address")?;
    let mut word = vec![0u8; 32];
    word[12..32].copy_from_slice(&address);
    Ok(word)
}

// --- content-mint calldata assembly (PC2 elacity-creator/app.js fidelity) -----------
//
// Reproduces `mint(string _uri, uint16 opType, bytes opRawData, bytes sellRawData)` and
// its `opRawData`/`sellRawData` payloads byte-for-byte against the Solidity ABI spec.
// PURE: no chain RPC, no keys. The selector is supplied (configured) exactly like the
// `has_access_by_content_id` selector — keccak is not computed in-capsule.

/// The canonical mint signature (selector = `keccak256(MINT_SIGNATURE)[..4]`, supplied
/// by config — not computed here).
pub(super) const MINT_SIGNATURE: &str = "mint(string,uint16,bytes,bytes)";

/// A `uintN` (N<=128) right-aligned in a 32-byte word.
pub(super) fn abi_word_u128(value: u128) -> Vec<u8> {
    let mut word = vec![0u8; 32];
    word[16..32].copy_from_slice(&value.to_be_bytes());
    word
}

/// A `bytes16` left-aligned in a 32-byte word (data in the high 16 bytes, zero-padded
/// right) — the on-chain `contentId == KID`. `content_id` is `0x` + 32 hex.
pub(super) fn abi_word_bytes16(content_id: &str) -> Result<Vec<u8>, String> {
    let bytes = decode_hex(content_id, Some(16), "bytes16 contentId")?;
    let mut word = vec![0u8; 32];
    word[0..16].copy_from_slice(&bytes);
    Ok(word)
}

/// A full `uint256` from a base-10 string (prices/copies can exceed u64).
pub(super) fn abi_word_uint256_decimal(dec: &str) -> Result<Vec<u8>, String> {
    if dec.is_empty() || !dec.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("expected a base-10 integer, got {dec:?}"));
    }
    let mut word = [0u8; 32];
    for ch in dec.bytes() {
        let mut carry = (ch - b'0') as u32;
        for byte in word.iter_mut().rev() {
            let v = (*byte as u32) * 10 + carry;
            *byte = (v & 0xff) as u8;
            carry = v >> 8;
        }
        if carry != 0 {
            return Err("uint256 overflow".to_string());
        }
    }
    Ok(word.to_vec())
}

fn abi_encode_address_array(addrs: &[String]) -> Result<Vec<u8>, String> {
    let mut out = abi_word_usize(addrs.len());
    for a in addrs {
        out.extend_from_slice(&abi_word_address(a)?);
    }
    Ok(out)
}

fn abi_encode_uint_array_u64(vals: &[u64]) -> Vec<u8> {
    let mut out = abi_word_usize(vals.len());
    for v in vals {
        out.extend_from_slice(&abi_word_u128(*v as u128));
    }
    out
}

fn abi_encode_uint_array_decimal(vals: &[String]) -> Result<Vec<u8>, String> {
    let mut out = abi_word_usize(vals.len());
    for v in vals {
        out.extend_from_slice(&abi_word_uint256_decimal(v)?);
    }
    Ok(out)
}

/// FREE-case `opRawData = abi.encode(bytes16 contentId)` (app.js:4941).
pub(super) fn encode_op_raw_free(content_id: &str) -> Result<Vec<u8>, String> {
    abi_word_bytes16(content_id)
}

/// PAID-case `opRawData = abi.encode(bytes16, string metadataUri, address[], uint256[]
/// roleTypes, uint256[] amounts[, uint16 resellerCut])` (app.js:1620/1627). The trailing
/// `uint16` is present iff `reseller_cut` is set (BUY_AND_RESELL).
pub(super) fn encode_op_raw_paid(
    content_id: &str,
    metadata_uri: &str,
    addresses: &[String],
    role_types: &[u64],
    amounts: &[String],
    reseller_cut: Option<u16>,
) -> Result<Vec<u8>, String> {
    if addresses.len() != role_types.len() || addresses.len() != amounts.len() {
        return Err("opRawData addresses/roleTypes/amounts must be equal length".to_string());
    }
    if addresses.is_empty() {
        return Err("opRawData requires at least one payee".to_string());
    }
    let num_head_words = if reseller_cut.is_some() { 6 } else { 5 };
    let head_size = num_head_words * 32;

    let enc_uri = abi_encode_string(metadata_uri.as_bytes());
    let enc_addr = abi_encode_address_array(addresses)?;
    let enc_roles = abi_encode_uint_array_u64(role_types);
    let enc_amts = abi_encode_uint_array_decimal(amounts)?;

    let uri_off = head_size;
    let addr_off = uri_off + enc_uri.len();
    let role_off = addr_off + enc_addr.len();
    let amt_off = role_off + enc_roles.len();

    let mut out = abi_word_bytes16(content_id)?; // [0] static bytes16
    out.extend_from_slice(&abi_word_usize(uri_off)); // [1] string offset
    out.extend_from_slice(&abi_word_usize(addr_off)); // [2] address[] offset
    out.extend_from_slice(&abi_word_usize(role_off)); // [3] uint256[] offset
    out.extend_from_slice(&abi_word_usize(amt_off)); // [4] uint256[] offset
    if let Some(cut) = reseller_cut {
        out.extend_from_slice(&abi_word_u128(cut as u128)); // [5] static uint16
    }
    out.extend_from_slice(&enc_uri);
    out.extend_from_slice(&enc_addr);
    out.extend_from_slice(&enc_roles);
    out.extend_from_slice(&enc_amts);
    Ok(out)
}

/// `sellRawData = abi.encode(uint256 copies, uint256 priceWei, address payToken)`
/// (encodeSellRawData, app.js:1633).
pub(super) fn encode_sell_raw_data(
    copies: &str,
    price_wei: &str,
    pay_token: &str,
) -> Result<Vec<u8>, String> {
    let mut out = abi_word_uint256_decimal(copies)?;
    out.extend_from_slice(&abi_word_uint256_decimal(price_wei)?);
    out.extend_from_slice(&abi_word_address(pay_token)?);
    Ok(out)
}

/// Outer `mint(string,uint16,bytes,bytes)` calldata: `selector ‖ head ‖ tail`. `_uri`,
/// `opRawData` and `sellRawData` are dynamic (offset words); `opType` is the one static
/// head word.
pub(super) fn encode_mint_calldata(
    selector: &str,
    uri: &str,
    op_type: u16,
    op_raw: &[u8],
    sell_raw: &[u8],
) -> Result<String, String> {
    let mut bytes = decode_hex(selector, Some(4), "mint function selector")?;
    let enc_uri = abi_encode_string(uri.as_bytes());
    let enc_op = abi_encode_bytes(op_raw);
    let enc_sell = abi_encode_bytes(sell_raw);

    let head = 4 * 32;
    let uri_off = head;
    let op_off = uri_off + enc_uri.len();
    let sell_off = op_off + enc_op.len();

    bytes.extend_from_slice(&abi_word_usize(uri_off));
    bytes.extend_from_slice(&abi_word_u128(op_type as u128));
    bytes.extend_from_slice(&abi_word_usize(op_off));
    bytes.extend_from_slice(&abi_word_usize(sell_off));
    bytes.extend_from_slice(&enc_uri);
    bytes.extend_from_slice(&enc_op);
    bytes.extend_from_slice(&enc_sell);
    Ok(format!("0x{}", encode_hex(&bytes)))
}

/// The canonical createChannel signature (selector = `keccak256(..)[..4]` = `0xc384baa2`,
/// supplied by config — not computed here).
pub(super) const CREATE_CHANNEL_SIGNATURE: &str = "createChannel(uint8,uint8,string,string,bytes)";

/// Outer `createChannel(uint8,uint8,string,string,bytes)` calldata: `selector ‖ head ‖ tail`.
/// `channelType`/`scope` are static head words; `name`, `tokenURI`, `data` are dynamic
/// (offset words). PURE: no chain RPC, no keys.
pub(super) fn encode_create_channel_calldata(
    selector: &str,
    channel_type: u8,
    scope: u8,
    name: &str,
    token_uri: &str,
    data: &[u8],
) -> Result<String, String> {
    let mut bytes = decode_hex(selector, Some(4), "createChannel selector")?;
    let enc_name = abi_encode_string(name.as_bytes());
    let enc_uri = abi_encode_string(token_uri.as_bytes());
    let enc_data = abi_encode_bytes(data);

    let head = 5 * 32;
    let name_off = head;
    let uri_off = name_off + enc_name.len();
    let data_off = uri_off + enc_uri.len();

    bytes.extend_from_slice(&abi_word_u128(channel_type as u128));
    bytes.extend_from_slice(&abi_word_u128(scope as u128));
    bytes.extend_from_slice(&abi_word_usize(name_off));
    bytes.extend_from_slice(&abi_word_usize(uri_off));
    bytes.extend_from_slice(&abi_word_usize(data_off));
    bytes.extend_from_slice(&enc_name);
    bytes.extend_from_slice(&enc_uri);
    bytes.extend_from_slice(&enc_data);
    Ok(format!("0x{}", encode_hex(&bytes)))
}

/// The canonical `setApprovalForAll` signature (selector = `keccak256(..)[..4]` = `0xa22cb465`,
/// supplied by config — not computed here). PC2's 2nd mint tx grants the channel's authority
/// gateway operator rights on the per-asset operative contract, so the asset is tradable.
pub(super) const SET_APPROVAL_FOR_ALL_SIGNATURE: &str = "setApprovalForAll(address,bool)";

/// `setApprovalForAll(address operator, bool approved)` calldata: `selector ‖ address ‖ bool`,
/// both static words. PURE: no chain RPC, no keys (selector supplied, not computed).
pub(super) fn encode_set_approval_for_all_calldata(
    selector: &str,
    operator: &str,
    approved: bool,
) -> Result<String, String> {
    let mut bytes = decode_hex(selector, Some(4), "setApprovalForAll selector")?;
    bytes.extend_from_slice(&abi_word_address(operator)?);
    let mut bool_word = vec![0u8; 32];
    bool_word[31] = u8::from(approved);
    bytes.extend_from_slice(&bool_word);
    Ok(format!("0x{}", encode_hex(&bytes)))
}

/// `isApprovedForAll(address account, address operator)` calldata: `selector ‖ address ‖
/// address`. PURE: no chain RPC, no keys (selector supplied, not computed).
pub(super) fn encode_is_approved_for_all_calldata(
    selector: &str,
    account: &str,
    operator: &str,
) -> Result<String, String> {
    let mut bytes = decode_hex(selector, Some(4), "isApprovedForAll selector")?;
    bytes.extend_from_slice(&abi_word_address(account)?);
    bytes.extend_from_slice(&abi_word_address(operator)?);
    Ok(format!("0x{}", encode_hex(&bytes)))
}

/// Decode one `AssetCreated(address indexed _to, address indexed _channel, uint256 _tokenId,
/// string _tokenUri, uint16 _opType, address indexed opContract)` log into `(operative,
/// token_id_hex, block_number, log_index)`. `opContract` is the 4th indexed topic; `_tokenId`
/// is the first non-indexed word of `data`. Returns `None` on a malformed/foreign entry.
pub(super) fn decode_asset_created_log(entry: &Value) -> Option<(String, String, u64, u64)> {
    let topics = entry.get("topics").and_then(Value::as_array)?;
    if topics.len() < 4 {
        return None;
    }
    let op_topic = topics.get(3)?.as_str()?;
    let op_word = decode_hex(op_topic, Some(32), "opContract topic").ok()?;
    let operative = word_to_address(&op_word).ok()?;
    let data = entry.get("data").and_then(Value::as_str)?;
    let data_bytes = decode_hex(data, None, "log data").ok()?;
    if data_bytes.len() < 32 {
        return None;
    }
    let token_id = format!("0x{}", encode_hex(&data_bytes[0..32]));
    let block_number = entry
        .get("blockNumber")
        .and_then(Value::as_str)
        .and_then(|value| parse_hex_u64(value).ok())
        .unwrap_or(0);
    let log_index = entry
        .get("logIndex")
        .and_then(Value::as_str)
        .and_then(|value| parse_hex_u64(value).ok())
        .unwrap_or(0);
    Some((operative, token_id, block_number, log_index))
}

/// Decode the leading `bytes16` contentId (KID) from a `mint(string,uint16,bytes,bytes)`
/// calldata. `opRawData` is argument #2 (a dynamic `bytes`); its payload always begins with
/// the abi-encoded `bytes16 contentId` (left-aligned in the first word), so the first 16 bytes
/// of the payload ARE the KID — for both the FREE and PAID layouts. Returns `0x<32 hex>`.
/// `None` on any selector/offset/length mismatch (the caller treats that as "not this asset").
pub(super) fn decode_mint_content_id(input: &str) -> Option<String> {
    let bytes = decode_hex(input, None, "tx input").ok()?;
    // selector (4) + at least 4 head words (tokenURI off, opType, opRawData off, sellRawData off).
    if bytes.len() < 4 + 4 * 32 {
        return None;
    }
    let args = &bytes[4..];
    // The 3rd head word (bytes 64..96) is the offset to opRawData, relative to args start.
    let op_off = word_to_usize(&args[64..96])?;
    let len_pos = op_off.checked_add(32)?;
    let len = word_to_usize(args.get(op_off..len_pos)?)?;
    let payload = args.get(len_pos..len_pos.checked_add(len)?)?;
    if payload.len() < 16 {
        return None;
    }
    Some(format!("0x{}", encode_hex(&payload[0..16])))
}

/// Read a 32-byte ABI word as a `usize` offset/length. Fail-closed if it does not fit in a
/// `usize` or has non-zero high bytes beyond the low 8 (a malformed/hostile calldata word).
fn word_to_usize(word: &[u8]) -> Option<usize> {
    if word.len() != 32 || word[..24].iter().any(|b| *b != 0) {
        return None;
    }
    let mut v = [0u8; 8];
    v.copy_from_slice(&word[24..32]);
    usize::try_from(u64::from_be_bytes(v)).ok()
}

/// Normalise a `bytes16` content id to lowercase 32-hex (no `0x`). `None` if it is not a clean
/// 16-byte hex value — so a caller can pin only on a real KID and fail closed otherwise.
pub(super) fn normalize_content_id_bytes16(content_id: &str) -> Option<String> {
    let bytes = decode_hex(content_id, Some(16), "bytes16 contentId").ok()?;
    Some(encode_hex(&bytes))
}

/// An EVM address as a 32-byte indexed-log topic (left-zero-padded), `0x`-prefixed.
pub(super) fn address_topic(address: &str) -> Result<String, String> {
    let word = abi_word_address(address)?;
    Ok(format!("0x{}", encode_hex(&word)))
}

/// Decode an EVM address from a 32-byte ABI word (the low 20 bytes). Fail-closed if the
/// high 12 bytes are non-zero (not a clean address word).
pub(super) fn word_to_address(word: &[u8]) -> Result<String, String> {
    if word.len() != 32 {
        return Err("address word must be 32 bytes".to_string());
    }
    if word[..12].iter().any(|byte| *byte != 0) {
        return Err("address word has non-zero high bytes".to_string());
    }
    Ok(format!("0x{}", encode_hex(&word[12..32])))
}

/// Decode a `uint8` from a 32-byte ABI word / indexed topic (the low byte; the rest zero).
pub(super) fn word_to_u8(word: &[u8]) -> Result<u8, String> {
    if word.len() != 32 {
        return Err("uint8 word must be 32 bytes".to_string());
    }
    if word[..31].iter().any(|byte| *byte != 0) {
        return Err("uint8 word has non-zero high bytes".to_string());
    }
    Ok(word[31])
}

/// Decode one `ChannelCreated` `eth_getLogs` entry into `{ address, channel_type, scope,
/// block_number }`. The channel address is the first word of `data` (non-indexed); the
/// channelType/scope are the 1st/2nd indexed topics. Fail-closed on a malformed entry.
pub(super) fn decode_channel_log(entry: &Value) -> Result<Value, String> {
    let data = entry
        .get("data")
        .and_then(Value::as_str)
        .ok_or("log entry missing data")?;
    let data_bytes = decode_hex(data, None, "log data")?;
    if data_bytes.len() < 32 {
        return Err("log data too short for a channel address word".to_string());
    }
    let channel = word_to_address(&data_bytes[0..32])?;
    let topics = entry
        .get("topics")
        .and_then(Value::as_array)
        .ok_or("log entry missing topics")?;
    let topic_u8 = |idx: usize| -> Option<u8> {
        let topic = topics.get(idx)?.as_str()?;
        let word = decode_hex(topic, Some(32), "topic").ok()?;
        word_to_u8(&word).ok()
    };
    let channel_type = topic_u8(1);
    let scope = topic_u8(2);
    let block_number = entry
        .get("blockNumber")
        .and_then(Value::as_str)
        .and_then(|value| parse_hex_u64(value).ok());
    Ok(json!({
        "address": channel,
        "channel_type": channel_type,
        "scope": scope,
        "block_number": block_number,
    }))
}

pub(super) fn decode_evm_bool(value: &Value) -> Result<bool, String> {
    let value = value
        .as_str()
        .ok_or_else(|| "EVM bool result must be hex string".to_string())?;
    let bytes = decode_hex(value, Some(32), "EVM bool result")?;
    if bytes[..31].iter().any(|byte| *byte != 0) {
        return Err("EVM bool result has non-zero high bytes".to_string());
    }
    match bytes[31] {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err("EVM bool result must be 0 or 1".to_string()),
    }
}

pub(super) fn decode_erc1271_magic_value(value: &Value) -> Result<String, String> {
    let value = value
        .as_str()
        .ok_or_else(|| "ERC-1271 result must be hex string".to_string())?;
    let bytes = decode_hex(value, None, "ERC-1271 result")?;
    if bytes.len() < 4 {
        return Err("ERC-1271 result must contain bytes4 magic value".to_string());
    }
    Ok(format!("0x{}", encode_hex(&bytes[..4])))
}
