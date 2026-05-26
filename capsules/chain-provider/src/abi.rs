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
