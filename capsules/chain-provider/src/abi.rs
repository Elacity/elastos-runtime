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
