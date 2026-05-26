use serde_json::Value;
use sha3::{Digest, Keccak256};

pub(crate) fn payload_str<'a>(payload: &'a Value, field: &str) -> Result<&'a str, String> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("transaction intent missing {field}"))
}

pub(crate) fn payload_u64(payload: &Value, field: &str) -> Result<u64, String> {
    payload
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("transaction intent missing numeric {field}"))
}

pub(crate) fn payload_evm_address_bytes(payload: &Value, field: &str) -> Result<Vec<u8>, String> {
    let value = payload_str(payload, field)?;
    elastos_auth::validate_evm_address(value)?;
    hex_prefixed_bytes(value, Some(20), field)
}

pub(crate) fn payload_hex_bytes(payload: &Value, field: &str) -> Result<Vec<u8>, String> {
    hex_prefixed_bytes(payload_str(payload, field)?, None, field)
}

pub(crate) fn payload_quantity_bytes(payload: &Value, field: &str) -> Result<Vec<u8>, String> {
    hex_quantity_bytes(payload_str(payload, field)?, field)
}

pub(crate) fn hex_prefixed_bytes(
    value: &str,
    expected_bytes: Option<usize>,
    field: &str,
) -> Result<Vec<u8>, String> {
    let raw = value
        .strip_prefix("0x")
        .ok_or_else(|| format!("{field} must start with 0x"))?;
    if raw.len() % 2 != 0 || !raw.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!("{field} must be even-length hex"));
    }
    if let Some(expected_bytes) = expected_bytes {
        if raw.len() != expected_bytes * 2 {
            return Err(format!("{field} must be {expected_bytes} bytes"));
        }
    }
    hex::decode(raw).map_err(|err| err.to_string())
}

pub(crate) fn hex_quantity_bytes(value: &str, field: &str) -> Result<Vec<u8>, String> {
    let raw = value
        .strip_prefix("0x")
        .ok_or_else(|| format!("{field} must start with 0x"))?;
    if raw.is_empty() || !raw.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!("{field} must be a hex quantity"));
    }
    if raw == "0" {
        return Ok(Vec::new());
    }
    if raw.len() > 1 && raw.starts_with('0') {
        return Err(format!("{field} must be a canonical hex quantity"));
    }
    let padded = if raw.len() % 2 == 0 {
        raw.to_string()
    } else {
        format!("0{raw}")
    };
    let bytes = hex::decode(padded).map_err(|err| err.to_string())?;
    Ok(trim_integer_bytes(&bytes).to_vec())
}

pub(crate) fn trim_integer_bytes(bytes: &[u8]) -> &[u8] {
    match bytes.iter().position(|byte| *byte != 0) {
        Some(index) => &bytes[index..],
        None => &[],
    }
}

pub(crate) fn left_pad_32(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let len = bytes.len().min(32);
    out[32 - len..].copy_from_slice(&bytes[bytes.len() - len..]);
    out
}

pub(crate) fn keccak256_bytes(bytes: &[u8]) -> [u8; 32] {
    Keccak256::digest(bytes).into()
}
