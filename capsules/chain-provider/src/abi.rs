use super::*;

pub(super) const PROTECTED_CONTENT_OPERATIVE_SELECTOR: &str = "0x70611dd2";
pub(super) const PROTECTED_CONTENT_LISTINGS_SELECTOR: &str = "0x6bd3a64b";
pub(super) const PROTECTED_CONTENT_PAYMENT_PROCESSOR_SELECTOR: &str = "0xf1c6bdf8";
pub(super) const PROTECTED_CONTENT_BUY_ACCESS_NATIVE_SELECTOR: &str = "0xf7580ad9";
pub(super) const PROTECTED_CONTENT_BUY_ACCESS_ERC20_SELECTOR: &str = "0x0ede2294";
pub(super) const ACCESS_TOKEN_ID_HEX: &str = "0x1";
pub(super) const PROTECTED_CONTENT_PURCHASE_QUANTITY_HEX: &str = "0x1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProtectedContentAssetCreatedLog {
    pub(super) creator: String,
    pub(super) ledger: String,
    pub(super) operative: String,
    pub(super) token_id: String,
    pub(super) token_uri: String,
    pub(super) op_type_code: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProtectedContentListingRead {
    pub(super) quantity: String,
    pub(super) price: String,
    pub(super) pay_token: String,
}

pub(super) fn encode_has_access_by_content_id_call(
    selector: &str,
    content_access_id: &[u8; 16],
    subject: &str,
) -> Result<String, String> {
    let mut bytes = decode_hex(selector, Some(4), "EVM function selector")?;
    bytes.extend_from_slice(&abi_word_address(subject)?);
    bytes.extend_from_slice(&abi_word_bytes16(content_access_id));

    Ok(format!("0x{}", encode_hex(&bytes)))
}

pub(super) fn encode_protected_content_creator_mint_call(
    selector: &str,
    token_uri: &str,
    op_type_code: u16,
    op_raw: &[u8],
    sell_raw: &[u8],
) -> Result<String, String> {
    let mut bytes = decode_hex(selector, Some(4), "mint function selector")?;
    let enc_uri = abi_encode_string(token_uri.as_bytes());
    let enc_op = abi_encode_bytes(op_raw);
    let enc_sell = abi_encode_bytes(sell_raw);

    let head = 4 * 32;
    let uri_off = head;
    let op_off = uri_off + enc_uri.len();
    let sell_off = op_off + enc_op.len();

    bytes.extend_from_slice(&abi_word_usize(uri_off));
    bytes.extend_from_slice(&abi_word_u128(op_type_code as u128));
    bytes.extend_from_slice(&abi_word_usize(op_off));
    bytes.extend_from_slice(&abi_word_usize(sell_off));
    bytes.extend_from_slice(&enc_uri);
    bytes.extend_from_slice(&enc_op);
    bytes.extend_from_slice(&enc_sell);
    Ok(format!("0x{}", encode_hex(&bytes)))
}

pub(super) fn encode_protected_content_mint_op_raw_paid(
    content_access_id: &[u8; 16],
    metadata_uri: &str,
    addresses: &[String],
    role_types: &[u64],
    amounts: &[String],
    reseller_cut: Option<u16>,
) -> Result<Vec<u8>, String> {
    if addresses.len() != role_types.len() || addresses.len() != amounts.len() {
        return Err("op_raw addresses/role_types/amounts must be equal length".to_string());
    }
    if addresses.is_empty() {
        return Err("op_raw requires at least one payee".to_string());
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

    let mut out = abi_word_bytes16(content_access_id);
    out.extend_from_slice(&abi_word_usize(uri_off));
    out.extend_from_slice(&abi_word_usize(addr_off));
    out.extend_from_slice(&abi_word_usize(role_off));
    out.extend_from_slice(&abi_word_usize(amt_off));
    if let Some(cut) = reseller_cut {
        out.extend_from_slice(&abi_word_u128(cut as u128));
    }
    out.extend_from_slice(&enc_uri);
    out.extend_from_slice(&enc_addr);
    out.extend_from_slice(&enc_roles);
    out.extend_from_slice(&enc_amts);
    Ok(out)
}

pub(super) fn encode_protected_content_sell_raw_data(
    copies: &str,
    price: &str,
    pay_token: &str,
) -> Result<Vec<u8>, String> {
    let mut out = abi_word_hex_quantity(copies, "copies")?;
    out.extend_from_slice(&abi_word_hex_quantity(price, "price")?);
    out.extend_from_slice(&abi_word_address(pay_token)?);
    Ok(out)
}

pub(super) fn encode_authority_gateway_buy_access_call(
    selector: &str,
    seller: &str,
    ledger: &str,
    token_id: &str,
    quantity: &str,
    price: &str,
    pay_token: Option<&str>,
) -> Result<String, String> {
    let mut bytes = decode_hex(selector, Some(4), "EVM function selector")?;
    bytes.extend_from_slice(&abi_word_address(seller)?);
    bytes.extend_from_slice(&abi_word_address(ledger)?);
    bytes.extend_from_slice(&abi_word_hex_quantity(token_id, "token_id")?);
    bytes.extend_from_slice(&abi_word_hex_quantity(quantity, "quantity")?);
    bytes.extend_from_slice(&abi_word_hex_quantity(price, "price")?);
    if let Some(pay_token) = pay_token {
        bytes.extend_from_slice(&abi_word_address(pay_token)?);
    }
    Ok(format!("0x{}", encode_hex(&bytes)))
}

pub(super) fn encode_authority_gateway_operative_call(
    ledger: &str,
    token_id: &str,
) -> Result<String, String> {
    let mut bytes = decode_hex(
        PROTECTED_CONTENT_OPERATIVE_SELECTOR,
        Some(4),
        "operative selector",
    )?;
    bytes.extend_from_slice(&abi_word_address(ledger)?);
    bytes.extend_from_slice(&abi_word_hex_quantity(token_id, "token_id")?);
    Ok(format!("0x{}", encode_hex(&bytes)))
}

pub(super) fn encode_authority_gateway_listing_call(
    operative: &str,
    seller: &str,
) -> Result<String, String> {
    let mut bytes = decode_hex(
        PROTECTED_CONTENT_LISTINGS_SELECTOR,
        Some(4),
        "listings selector",
    )?;
    bytes.extend_from_slice(&abi_word_address(operative)?);
    bytes.extend_from_slice(&abi_word_hex_quantity(
        ACCESS_TOKEN_ID_HEX,
        "access_token_id",
    )?);
    bytes.extend_from_slice(&abi_word_address(seller)?);
    Ok(format!("0x{}", encode_hex(&bytes)))
}

pub(super) fn encode_operatives_payment_processor_call() -> Result<String, String> {
    let bytes = decode_hex(
        PROTECTED_CONTENT_PAYMENT_PROCESSOR_SELECTOR,
        Some(4),
        "paymentProcessor selector",
    )?;
    Ok(format!("0x{}", encode_hex(&bytes)))
}

pub(super) fn encode_erc20_approve_call(spender: &str, amount: &str) -> Result<String, String> {
    let mut bytes = vec![0x09, 0x5e, 0xa7, 0xb3];
    bytes.extend_from_slice(&abi_word_address(spender)?);
    bytes.extend_from_slice(&abi_word_hex_quantity(amount, "approval_amount")?);
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

pub(super) fn abi_word_u128(value: u128) -> Vec<u8> {
    let mut word = vec![0u8; 32];
    word[16..32].copy_from_slice(&value.to_be_bytes());
    word
}

fn abi_encode_address_array(addrs: &[String]) -> Result<Vec<u8>, String> {
    let mut out = abi_word_usize(addrs.len());
    for address in addrs {
        out.extend_from_slice(&abi_word_address(address)?);
    }
    Ok(out)
}

fn abi_encode_uint_array_u64(values: &[u64]) -> Vec<u8> {
    let mut out = abi_word_usize(values.len());
    for value in values {
        out.extend_from_slice(&abi_word_u128(*value as u128));
    }
    out
}

fn abi_encode_uint_array_decimal(values: &[String]) -> Result<Vec<u8>, String> {
    let mut out = abi_word_usize(values.len());
    for value in values {
        out.extend_from_slice(&abi_word_hex_quantity(value, "amount")?);
    }
    Ok(out)
}

pub(super) fn abi_word_address(address: &str) -> Result<Vec<u8>, String> {
    let address = decode_hex(address, Some(20), "EVM address")?;
    let mut word = vec![0u8; 32];
    word[12..32].copy_from_slice(&address);
    Ok(word)
}

pub(super) fn abi_word_bytes16(value: &[u8; 16]) -> Vec<u8> {
    let mut word = vec![0u8; 32];
    word[..16].copy_from_slice(value);
    word
}

pub(super) fn decode_evm_address_word(value: &Value, label: &str) -> Result<String, String> {
    let value = value
        .as_str()
        .ok_or_else(|| format!("{label} result must be hex string"))?;
    let bytes = decode_hex(value, Some(32), label)?;
    word_to_address(&bytes)
}

pub(super) fn decode_protected_content_listing(
    value: &Value,
) -> Result<ProtectedContentListingRead, String> {
    let value = value
        .as_str()
        .ok_or_else(|| "listing result must be hex string".to_string())?;
    let bytes = decode_hex(value, None, "listing result")?;
    if bytes.len() != 96 {
        return Err("listing result must contain exactly three ABI words".to_string());
    }
    Ok(ProtectedContentListingRead {
        quantity: normalize_hex_quantity_bytes(&bytes[0..32]),
        price: normalize_hex_quantity_bytes(&bytes[32..64]),
        pay_token: word_to_address(&bytes[64..96])?,
    })
}

pub(super) fn decode_protected_content_asset_created_log(
    entry: &Value,
) -> Result<ProtectedContentAssetCreatedLog, String> {
    let topics = entry
        .get("topics")
        .and_then(Value::as_array)
        .ok_or_else(|| "AssetCreated log topics missing".to_string())?;
    if topics.len() != 4 {
        return Err("AssetCreated log must contain exactly four topics".to_string());
    }
    let creator = topic_to_address(topics.get(1), "creator topic")?;
    let ledger = topic_to_address(topics.get(2), "channel topic")?;
    let operative = topic_to_address(topics.get(3), "operative topic")?;
    let data = entry
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| "AssetCreated log data missing".to_string())?;
    let data_bytes = decode_hex(data, None, "AssetCreated log data")?;
    if data_bytes.len() < 96 {
        return Err("AssetCreated log data is truncated".to_string());
    }
    let token_id = normalize_hex_quantity_bytes(&data_bytes[0..32]);
    let token_uri_offset = usize_from_word(&data_bytes[32..64], "AssetCreated tokenUri offset")?;
    if token_uri_offset != 96 {
        return Err("AssetCreated tokenUri offset must be exactly 96".to_string());
    }
    let op_type_code = u16_from_word(&data_bytes[64..96], "AssetCreated opType")?;
    let token_uri_length_end = token_uri_offset
        .checked_add(32)
        .ok_or_else(|| "AssetCreated tokenUri length overflows".to_string())?;
    if token_uri_length_end > data_bytes.len() {
        return Err("AssetCreated tokenUri length word is out of bounds".to_string());
    }
    let token_uri_length = usize_from_word(
        &data_bytes[token_uri_offset..token_uri_length_end],
        "AssetCreated tokenUri",
    )?;
    let padded_token_uri_chunks = token_uri_length
        .checked_add(31)
        .ok_or_else(|| "AssetCreated tokenUri length overflows".to_string())?
        / 32;
    let padded_token_uri_length = padded_token_uri_chunks
        .checked_mul(32)
        .ok_or_else(|| "AssetCreated tokenUri length overflows".to_string())?;
    let expected_total = token_uri_length_end
        .checked_add(padded_token_uri_length)
        .ok_or_else(|| "AssetCreated tokenUri length overflows".to_string())?;
    if data_bytes.len() != expected_total {
        return Err("AssetCreated log data length is non-canonical".to_string());
    }
    let token_uri_value_end = token_uri_length_end
        .checked_add(token_uri_length)
        .ok_or_else(|| "AssetCreated tokenUri length overflows".to_string())?;
    if data_bytes[token_uri_value_end..expected_total]
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err("AssetCreated tokenUri padding must be zero".to_string());
    }
    let token_uri = decode_abi_string(&data_bytes, token_uri_offset, "AssetCreated tokenUri")?;
    Ok(ProtectedContentAssetCreatedLog {
        creator,
        ledger,
        operative,
        token_id,
        token_uri,
        op_type_code,
    })
}

pub(super) fn abi_word_hex_quantity(value: &str, label: &str) -> Result<Vec<u8>, String> {
    validate_hex_quantity(value, label)?;
    let raw = decode_hex_quantity(value, label)?;
    if raw.len() > 32 {
        return Err(format!("{label} must fit in uint256"));
    }
    let mut word = vec![0u8; 32];
    let start = 32 - raw.len();
    word[start..].copy_from_slice(&raw);
    Ok(word)
}

pub(super) fn normalize_hex_quantity(value: &str, label: &str) -> Result<String, String> {
    let bytes = decode_hex_quantity(value, label)?;
    Ok(normalize_hex_quantity_bytes(&bytes))
}

fn decode_hex_quantity(value: &str, label: &str) -> Result<Vec<u8>, String> {
    validate_hex_quantity(value, label)?;
    let raw = value
        .strip_prefix("0x")
        .ok_or_else(|| format!("{label} must start with 0x"))?;
    let padded = if raw.len() % 2 == 0 {
        raw.to_string()
    } else {
        format!("0{raw}")
    };
    let mut decoded = Vec::with_capacity(padded.len() / 2);
    let chars = padded.as_bytes();
    for chunk in chars.chunks_exact(2) {
        let high = hex_value(chunk[0]).ok_or_else(|| format!("{label} must be hex"))?;
        let low = hex_value(chunk[1]).ok_or_else(|| format!("{label} must be hex"))?;
        decoded.push((high << 4) | low);
    }
    Ok(decoded)
}

fn normalize_hex_quantity_bytes(bytes: &[u8]) -> String {
    let first_nonzero = bytes.iter().position(|byte| *byte != 0);
    match first_nonzero {
        Some(index) => {
            let mut hex = encode_hex(&bytes[index..]);
            if hex.is_empty() {
                hex.push('0');
            }
            if hex.starts_with('0') && hex.len() > 1 {
                format!("0x{}", hex.trim_start_matches('0'))
            } else {
                format!("0x{hex}")
            }
        }
        None => "0x0".to_string(),
    }
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

fn topic_to_address(value: Option<&Value>, label: &str) -> Result<String, String> {
    let value = value
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{label} missing"))?;
    let bytes = decode_hex(value, Some(32), label)?;
    word_to_address(&bytes)
}

fn word_to_address(word: &[u8]) -> Result<String, String> {
    if word.len() != 32 {
        return Err("address word must be 32 bytes".to_string());
    }
    if word[..12].iter().any(|byte| *byte != 0) {
        return Err("address word has non-zero high bytes".to_string());
    }
    Ok(format!("0x{}", encode_hex(&word[12..32])))
}

fn usize_from_word(word: &[u8], label: &str) -> Result<usize, String> {
    if word.len() != 32 {
        return Err(format!("{label} must be 32 bytes"));
    }
    if word[..24].iter().any(|byte| *byte != 0) {
        return Err(format!("{label} exceeds usize"));
    }
    let mut value = [0u8; 8];
    value.copy_from_slice(&word[24..32]);
    usize::try_from(u64::from_be_bytes(value)).map_err(|_| format!("{label} exceeds usize"))
}

fn u16_from_word(word: &[u8], label: &str) -> Result<u16, String> {
    if word.len() != 32 {
        return Err(format!("{label} must be 32 bytes"));
    }
    if word[..30].iter().any(|byte| *byte != 0) {
        return Err(format!("{label} must fit in uint16"));
    }
    Ok(u16::from_be_bytes([word[30], word[31]]))
}

fn decode_abi_string(data: &[u8], offset: usize, label: &str) -> Result<String, String> {
    let len_end = offset
        .checked_add(32)
        .ok_or_else(|| format!("{label} offset overflows"))?;
    if len_end > data.len() {
        return Err(format!("{label} length word is out of bounds"));
    }
    let length = usize_from_word(&data[offset..len_end], label)?;
    let value_end = len_end
        .checked_add(length)
        .ok_or_else(|| format!("{label} length overflows"))?;
    if value_end > data.len() {
        return Err(format!("{label} bytes are truncated"));
    }
    std::str::from_utf8(&data[len_end..value_end])
        .map(|value| value.to_string())
        .map_err(|_| format!("{label} must be UTF-8"))
}
