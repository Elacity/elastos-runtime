use super::*;
use elastos_protected_content_contracts::{CanonicalContract, RuntimeOperationIssuerKeyV1};
use sha2::Digest;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn validate_network_id(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 64 {
        return Err("network id must be 1-64 characters".to_string());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err("network id must use lowercase letters, digits, and hyphens".to_string());
    }
    Ok(())
}

pub(super) fn validate_evm_address(value: &str) -> Result<(), String> {
    validate_hex(value, Some(20), "EVM address")
}

pub(super) fn validate_bitcoin_rest_address(value: &str) -> Result<(), String> {
    if !(14..=90).contains(&value.len()) {
        return Err("Bitcoin address length is invalid".to_string());
    }
    if !value.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return Err("Bitcoin address must be a path-safe base58 or bech32 address".to_string());
    }
    if value.starts_with("bc1") || value.starts_with('1') || value.starts_with('3') {
        return Ok(());
    }
    Err("Bitcoin address must be mainnet base58 or bech32".to_string())
}

pub(super) fn normalize_evm_address(value: &str) -> String {
    value.to_ascii_lowercase()
}

pub(super) fn validate_evm_hash(value: &str) -> Result<(), String> {
    validate_hex(value, Some(32), "EVM hash")
}

pub(super) fn validate_subject(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 256 {
        return Err("subject must be 1-256 characters".to_string());
    }
    if value
        .chars()
        .any(|ch| ch.is_ascii_whitespace() || ch.is_ascii_control() || ch == '/' || ch == '\\')
    {
        return Err("subject must be an opaque principal, DID, or account identifier".to_string());
    }
    Ok(())
}

pub(super) fn validate_hex(value: &str, bytes: Option<usize>, label: &str) -> Result<(), String> {
    let raw = value
        .strip_prefix("0x")
        .ok_or_else(|| format!("{label} must start with 0x"))?;
    if raw.len() % 2 != 0 || !raw.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!("{label} must be even-length hex"));
    }
    if let Some(bytes) = bytes {
        let expected = bytes * 2;
        if raw.len() != expected {
            return Err(format!("{label} must be {bytes} bytes"));
        }
    }
    Ok(())
}

pub(super) fn validate_hex_quantity(value: &str, label: &str) -> Result<(), String> {
    let raw = value
        .strip_prefix("0x")
        .ok_or_else(|| format!("{label} must start with 0x"))?;
    if raw.is_empty() || !raw.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!("{label} must be a hex quantity"));
    }
    Ok(())
}

pub(super) fn validated_rpc_quantity(value: &Value, label: &str) -> Result<String, String> {
    let value = value
        .as_str()
        .ok_or_else(|| format!("{label} must be a hex quantity string"))?;
    validate_hex_quantity(value, label)?;
    Ok(value.to_string())
}

pub(super) fn validate_signed_transaction(value: &str) -> Result<(), String> {
    let raw = value
        .strip_prefix("0x")
        .ok_or_else(|| "signed transaction must start with 0x".to_string())?;
    if raw.len() < 2 || raw.len() % 2 != 0 || !raw.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("signed transaction must be even-length hex".to_string());
    }
    Ok(())
}

pub(super) fn decode_hex(
    value: &str,
    bytes: Option<usize>,
    label: &str,
) -> Result<Vec<u8>, String> {
    validate_hex(value, bytes, label)?;
    let raw = value.trim_start_matches("0x");
    let mut decoded = Vec::with_capacity(raw.len() / 2);
    let chars = raw.as_bytes();
    for chunk in chars.chunks_exact(2) {
        let high = hex_value(chunk[0]).ok_or_else(|| format!("{label} must be hex"))?;
        let low = hex_value(chunk[1]).ok_or_else(|| format!("{label} must be hex"))?;
        decoded.push((high << 4) | low);
    }
    Ok(decoded)
}

pub(super) fn decode_contract_hex<T: CanonicalContract>(
    value: &str,
    max_bytes: usize,
    label: &str,
) -> Result<T, String> {
    if value.strip_prefix("0x").is_none_or(|raw| {
        raw.chars()
            .any(|ch| !ch.is_ascii_lowercase() && !ch.is_ascii_digit())
    }) {
        return Err(format!("{label} must be lowercase 0x-prefixed hex"));
    }
    let bytes = decode_hex(value, None, label)?;
    if bytes.is_empty() || bytes.len() > max_bytes {
        return Err(format!("{label} size is invalid"));
    }
    T::from_canonical_bytes(&bytes).map_err(|err| err.to_string())
}

pub(super) fn parse_runtime_issuer(value: &Value) -> Result<RuntimeOperationIssuerKeyV1, String> {
    let value = value
        .as_str()
        .ok_or_else(|| "protected-content Runtime issuer must be a string".to_string())?;
    if value.strip_prefix("0x").is_none_or(|raw| {
        raw.chars()
            .any(|ch| !ch.is_ascii_lowercase() && !ch.is_ascii_digit())
    }) {
        return Err(
            "protected-content Runtime issuer must be lowercase 0x-prefixed hex".to_string(),
        );
    }
    let bytes = decode_hex(value, Some(32), "protected-content Runtime issuer")?;
    RuntimeOperationIssuerKeyV1::new(
        bytes
            .try_into()
            .map_err(|_| "protected-content Runtime issuer must be 32 bytes".to_string())?,
    )
    .map_err(|_| "protected-content Runtime issuer is invalid".to_string())
}

pub(super) fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(super) fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub(super) fn value_hash(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    bytes_hash(&bytes)
}

pub(super) fn bitcoin_balance_sats(body: &Value, field: &str) -> Result<u64, String> {
    let stats = body
        .get(field)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{field} missing"))?;
    let funded = stats
        .get("funded_txo_sum")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{field}.funded_txo_sum missing"))?;
    let spent = stats
        .get("spent_txo_sum")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{field}.spent_txo_sum missing"))?;
    Ok(funded.saturating_sub(spent))
}

pub(super) fn bytes_hash(bytes: &[u8]) -> String {
    format!("0x{}", encode_hex(&sha2::Sha256::digest(bytes)))
}

pub(super) fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn normalize_block_tag(value: Option<&str>) -> Result<String, String> {
    let value = value.unwrap_or("latest");
    match value {
        "latest" | "pending" | "earliest" => Ok(value.to_string()),
        _ => {
            let raw = value
                .strip_prefix("0x")
                .ok_or("block must be latest, pending, earliest, or hex quantity")?;
            if raw.is_empty() || !raw.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Err("block hex quantity is invalid".to_string());
            }
            Ok(value.to_string())
        }
    }
}

pub(super) fn validate_block_tag(value: &str, label: &str) -> Result<String, String> {
    match value {
        "latest" | "pending" | "earliest" | "safe" | "finalized" => Ok(value.to_string()),
        _ => {
            let raw = value
                .strip_prefix("0x")
                .ok_or_else(|| format!("{label} must be a block tag or hex quantity"))?;
            if raw.is_empty() || !raw.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Err(format!("{label} hex quantity is invalid"));
            }
            Ok(value.to_string())
        }
    }
}

pub(super) fn validate_evm_log_filter(filter: Value) -> Result<Value, String> {
    let object = filter
        .as_object()
        .ok_or_else(|| "log filter must be an object".to_string())?;
    let mut sanitized = serde_json::Map::new();
    for key in object.keys() {
        match key.as_str() {
            "fromBlock" | "toBlock" | "address" | "topics" | "blockHash" => {}
            _ => return Err(format!("unsupported log filter field {key}")),
        }
    }
    if object.contains_key("blockHash")
        && (object.contains_key("fromBlock") || object.contains_key("toBlock"))
    {
        return Err("blockHash cannot be combined with fromBlock or toBlock".to_string());
    }
    if let Some(value) = object.get("fromBlock").and_then(Value::as_str) {
        sanitized.insert(
            "fromBlock".to_string(),
            Value::String(validate_block_tag(value, "fromBlock")?),
        );
    }
    if let Some(value) = object.get("toBlock").and_then(Value::as_str) {
        sanitized.insert(
            "toBlock".to_string(),
            Value::String(validate_block_tag(value, "toBlock")?),
        );
    }
    if let Some(value) = object.get("blockHash").and_then(Value::as_str) {
        validate_evm_hash(value)?;
        sanitized.insert("blockHash".to_string(), Value::String(value.to_string()));
    }
    if let Some(value) = object.get("address") {
        match value {
            Value::String(address) => {
                validate_evm_address(address)?;
                sanitized.insert("address".to_string(), Value::String(address.to_string()));
            }
            Value::Array(addresses) => {
                if addresses.is_empty() || addresses.len() > 64 {
                    return Err("address filter must contain 1-64 addresses".to_string());
                }
                let mut sanitized_addresses = Vec::with_capacity(addresses.len());
                for address in addresses {
                    let address = address
                        .as_str()
                        .ok_or_else(|| "address filter entries must be strings".to_string())?;
                    validate_evm_address(address)?;
                    sanitized_addresses.push(Value::String(address.to_string()));
                }
                sanitized.insert("address".to_string(), Value::Array(sanitized_addresses));
            }
            _ => return Err("address filter must be a string or array".to_string()),
        }
    }
    if let Some(value) = object.get("topics") {
        sanitized.insert("topics".to_string(), validate_evm_log_topics(value)?);
    }
    if sanitized.len() > 5 {
        return Err("log filter is too large".to_string());
    }
    Ok(Value::Object(sanitized))
}

fn validate_evm_log_topics(value: &Value) -> Result<Value, String> {
    let topics = value
        .as_array()
        .ok_or_else(|| "topics filter must be an array".to_string())?;
    if topics.len() > 4 {
        return Err("topics filter must contain at most 4 topic positions".to_string());
    }
    let mut sanitized = Vec::with_capacity(topics.len());
    for topic in topics {
        match topic {
            Value::Null => sanitized.push(Value::Null),
            Value::String(topic) => {
                validate_evm_hash(topic)?;
                sanitized.push(Value::String(topic.to_string()));
            }
            Value::Array(alternatives) => {
                if alternatives.len() > 64 {
                    return Err("topic alternatives must contain at most 64 entries".to_string());
                }
                let mut sanitized_alternatives = Vec::with_capacity(alternatives.len());
                for alternative in alternatives {
                    match alternative {
                        Value::Null => sanitized_alternatives.push(Value::Null),
                        Value::String(topic) => {
                            validate_evm_hash(topic)?;
                            sanitized_alternatives.push(Value::String(topic.to_string()));
                        }
                        _ => {
                            return Err(
                                "topic alternatives must be null or topic hash strings".to_string()
                            )
                        }
                    }
                }
                sanitized.push(Value::Array(sanitized_alternatives));
            }
            _ => {
                return Err("topics entries must be null, topic hash strings, or arrays".to_string())
            }
        }
    }
    Ok(Value::Array(sanitized))
}

pub(super) fn parse_hex_u64(value: &str) -> Result<u64, String> {
    let raw = value
        .strip_prefix("0x")
        .ok_or_else(|| "hex quantity must start with 0x".to_string())?;
    u64::from_str_radix(raw, 16).map_err(|err| err.to_string())
}

pub(super) fn evm_sync_object(sync: serde_json::Map<String, Value>) -> Result<Value, String> {
    Ok(json!({
        "starting_block": evm_sync_quantity(&sync, "startingBlock")?,
        "current_block": evm_sync_quantity(&sync, "currentBlock")?,
        "highest_block": evm_sync_quantity(&sync, "highestBlock")?,
    }))
}

pub(super) fn evm_sync_quantity(
    sync: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<u64, String> {
    let value = sync
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("eth_syncing missing {key}"))?;
    parse_hex_u64(value).map_err(|err| format!("invalid {key}: {err}"))
}
