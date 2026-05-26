use super::payload::{hex_prefixed_bytes, keccak256_bytes, left_pad_32};
use elastos_auth::validate_evm_address;
use serde_json::Value;

pub(crate) fn eip712_payload_hash(payload: &Value) -> Result<[u8; 32], String> {
    let typed_data = typed_data_from_payload(payload)?;
    validate_eip712_typed_data_shape(&typed_data)?;
    eip712_hash_typed_data(&typed_data)
}

pub(crate) fn typed_data_from_payload(payload: &Value) -> Result<Value, String> {
    if let Some(canonical) = payload.get("typed_data_canonical").and_then(Value::as_str) {
        return serde_json::from_str(canonical).map_err(|err| format!("invalid typed data: {err}"));
    }
    payload
        .get("typed_data")
        .cloned()
        .ok_or_else(|| "Browser typed-data payload missing typed_data".to_string())
}

pub(crate) fn validate_eip712_typed_data_shape(typed_data: &Value) -> Result<(), String> {
    let types = typed_data
        .get("types")
        .and_then(Value::as_object)
        .ok_or_else(|| "typed data missing types".to_string())?;
    let primary_type = typed_data
        .get("primaryType")
        .and_then(Value::as_str)
        .ok_or_else(|| "typed data missing primaryType".to_string())?;
    if !types.contains_key(primary_type) {
        return Err("typed data primaryType is not declared".to_string());
    }
    typed_data
        .get("domain")
        .and_then(Value::as_object)
        .ok_or_else(|| "typed data missing domain".to_string())?;
    typed_data
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(|| "typed data missing message".to_string())?;
    Ok(())
}

pub(crate) fn eip712_hash_typed_data(typed_data: &Value) -> Result<[u8; 32], String> {
    let types = typed_data
        .get("types")
        .and_then(Value::as_object)
        .ok_or_else(|| "typed data missing types".to_string())?;
    let primary_type = typed_data
        .get("primaryType")
        .and_then(Value::as_str)
        .ok_or_else(|| "typed data missing primaryType".to_string())?;
    let domain = typed_data.get("domain").unwrap_or(&Value::Null);
    let message = typed_data.get("message").unwrap_or(&Value::Null);
    let domain_hash = eip712_hash_struct("EIP712Domain", domain, types)?;
    let message_hash = eip712_hash_struct(primary_type, message, types)?;
    let mut encoded = Vec::with_capacity(66);
    encoded.extend_from_slice(b"\x19\x01");
    encoded.extend_from_slice(&domain_hash);
    encoded.extend_from_slice(&message_hash);
    Ok(keccak256_bytes(&encoded))
}

pub(crate) fn eip712_hash_struct(
    type_name: &str,
    value: &Value,
    types: &serde_json::Map<String, Value>,
) -> Result<[u8; 32], String> {
    let fields = eip712_fields(type_name, types)?;
    let object = value
        .as_object()
        .ok_or_else(|| format!("typed data {type_name} value must be an object"))?;
    let mut encoded = Vec::with_capacity(32 * (fields.len() + 1));
    encoded.extend_from_slice(&keccak256_bytes(
        eip712_encode_type(type_name, types)?.as_bytes(),
    ));
    for field in fields {
        let field_name = eip712_field_name(field)?;
        let field_type = eip712_field_type(field)?;
        let field_value = object.get(field_name).unwrap_or(&Value::Null);
        encoded.extend_from_slice(&eip712_encode_value(field_type, field_value, types)?);
    }
    Ok(keccak256_bytes(&encoded))
}

pub(crate) fn eip712_encode_type(
    primary_type: &str,
    types: &serde_json::Map<String, Value>,
) -> Result<String, String> {
    let mut deps = Vec::<String>::new();
    collect_eip712_deps(primary_type, types, &mut deps)?;
    deps.retain(|dep| dep != primary_type);
    deps.sort();
    deps.dedup();
    let mut ordered = vec![primary_type.to_string()];
    ordered.extend(deps);
    ordered
        .iter()
        .map(|type_name| eip712_type_declaration(type_name, types))
        .collect::<Result<Vec<_>, _>>()
        .map(|items| items.join(""))
}

pub(crate) fn collect_eip712_deps(
    type_name: &str,
    types: &serde_json::Map<String, Value>,
    deps: &mut Vec<String>,
) -> Result<(), String> {
    for field in eip712_fields(type_name, types)? {
        let base = eip712_base_type(eip712_field_type(field)?);
        if base != type_name && types.contains_key(base) && !deps.iter().any(|dep| dep == base) {
            deps.push(base.to_string());
            collect_eip712_deps(base, types, deps)?;
        }
    }
    Ok(())
}

pub(crate) fn eip712_type_declaration(
    type_name: &str,
    types: &serde_json::Map<String, Value>,
) -> Result<String, String> {
    let fields = eip712_fields(type_name, types)?;
    let encoded_fields = fields
        .iter()
        .map(|field| {
            Ok(format!(
                "{} {}",
                eip712_field_type(field)?,
                eip712_field_name(field)?
            ))
        })
        .collect::<Result<Vec<String>, String>>()?;
    Ok(format!("{type_name}({})", encoded_fields.join(",")))
}

pub(crate) fn eip712_fields<'a>(
    type_name: &str,
    types: &'a serde_json::Map<String, Value>,
) -> Result<&'a Vec<Value>, String> {
    types
        .get(type_name)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("typed data type {type_name} is not declared"))
}

pub(crate) fn eip712_field_name(field: &Value) -> Result<&str, String> {
    field
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "typed data field missing name".to_string())
}

pub(crate) fn eip712_field_type(field: &Value) -> Result<&str, String> {
    field
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "typed data field missing type".to_string())
}

pub(crate) fn eip712_base_type(field_type: &str) -> &str {
    field_type.split('[').next().unwrap_or(field_type)
}

pub(crate) fn eip712_is_array(field_type: &str) -> bool {
    field_type.contains('[') && field_type.ends_with(']')
}

pub(crate) fn eip712_encode_value(
    field_type: &str,
    value: &Value,
    types: &serde_json::Map<String, Value>,
) -> Result<[u8; 32], String> {
    let base_type = eip712_base_type(field_type);
    if eip712_is_array(field_type) {
        let values = value
            .as_array()
            .ok_or_else(|| format!("typed data {field_type} value must be an array"))?;
        let mut encoded = Vec::with_capacity(values.len() * 32);
        for item in values {
            encoded.extend_from_slice(&eip712_encode_value(base_type, item, types)?);
        }
        return Ok(keccak256_bytes(&encoded));
    }
    if types.contains_key(base_type) {
        return eip712_hash_struct(base_type, value, types);
    }
    match base_type {
        "string" => value
            .as_str()
            .map(|text| keccak256_bytes(text.as_bytes()))
            .ok_or_else(|| "typed data string value must be a string".to_string()),
        "bytes" => {
            let bytes = value
                .as_str()
                .ok_or_else(|| "typed data bytes value must be hex".to_string())
                .and_then(|text| hex_prefixed_bytes(text, None, "typed data bytes"))?;
            Ok(keccak256_bytes(&bytes))
        }
        "bool" => {
            let mut out = [0u8; 32];
            if value
                .as_bool()
                .ok_or_else(|| "typed data bool value must be boolean".to_string())?
            {
                out[31] = 1;
            }
            Ok(out)
        }
        "address" => {
            let address = value
                .as_str()
                .ok_or_else(|| "typed data address value must be a string".to_string())?;
            validate_evm_address(address)?;
            let bytes = hex_prefixed_bytes(address, Some(20), "typed data address")?;
            Ok(left_pad_32(&bytes))
        }
        _ if base_type == "uint" || base_type.starts_with("uint") => {
            parse_json_uint256(value, "typed data uint")
        }
        _ if base_type == "int" || base_type.starts_with("int") => {
            parse_json_int256(value, "typed data int")
        }
        _ if base_type.starts_with("bytes") => {
            let size: usize = base_type[5..]
                .parse()
                .map_err(|_| "typed data fixed bytes size is invalid".to_string())?;
            if !(1..=32).contains(&size) {
                return Err("typed data fixed bytes size is invalid".to_string());
            }
            let bytes = value
                .as_str()
                .ok_or_else(|| "typed data fixed bytes value must be hex".to_string())
                .and_then(|text| hex_prefixed_bytes(text, Some(size), "typed data fixed bytes"))?;
            let mut out = [0u8; 32];
            out[..bytes.len()].copy_from_slice(&bytes);
            Ok(out)
        }
        _ => Err(format!("unsupported EIP-712 field type {field_type}")),
    }
}

pub(crate) fn parse_json_uint256(value: &Value, label: &str) -> Result<[u8; 32], String> {
    if let Some(raw) = value.as_str() {
        return parse_uint256(raw, label);
    }
    if let Some(raw) = value.as_u64() {
        return Ok(left_pad_32(&raw.to_be_bytes()));
    }
    Err(format!(
        "{label} value must be a string or unsigned integer"
    ))
}

pub(crate) fn parse_json_int256(value: &Value, label: &str) -> Result<[u8; 32], String> {
    if let Some(raw) = value.as_i64() {
        if raw < 0 {
            return parse_negative_i128(raw as i128);
        }
        return Ok(left_pad_32(&(raw as u64).to_be_bytes()));
    }
    if let Some(raw) = value.as_str() {
        if let Some(abs) = raw.strip_prefix('-') {
            let positive = parse_uint256(abs, label)?;
            return twos_complement_256(&positive);
        }
        return parse_uint256(raw, label);
    }
    Err(format!("{label} value must be a string or integer"))
}

pub(crate) fn parse_uint256(value: &str, label: &str) -> Result<[u8; 32], String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{label} value is empty"));
    }
    if let Some(raw) = value.strip_prefix("0x") {
        if raw.is_empty() || raw.len() > 64 || !raw.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(format!("{label} hex value is invalid"));
        }
        let padded = if raw.len() % 2 == 0 {
            raw.to_string()
        } else {
            format!("0{raw}")
        };
        let bytes = hex::decode(padded).map_err(|err| err.to_string())?;
        return Ok(left_pad_32(&bytes));
    }
    if !value.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(format!("{label} decimal value is invalid"));
    }
    let mut out = [0u8; 32];
    for digit in value.bytes().map(|byte| byte - b'0') {
        mul_u256_small(&mut out, 10)?;
        add_u256_small(&mut out, digit)?;
    }
    Ok(out)
}

pub(crate) fn parse_negative_i128(value: i128) -> Result<[u8; 32], String> {
    let abs = value
        .checked_abs()
        .ok_or_else(|| "typed data int value is out of range".to_string())?;
    let positive = parse_uint256(&abs.to_string(), "typed data int")?;
    twos_complement_256(&positive)
}

pub(crate) fn twos_complement_256(value: &[u8; 32]) -> Result<[u8; 32], String> {
    let mut out = [0u8; 32];
    for (target, byte) in out.iter_mut().zip(value.iter()) {
        *target = !*byte;
    }
    add_u256_small(&mut out, 1)?;
    Ok(out)
}

pub(crate) fn mul_u256_small(value: &mut [u8; 32], factor: u8) -> Result<(), String> {
    let mut carry = 0u16;
    for byte in value.iter_mut().rev() {
        let next = (*byte as u16) * (factor as u16) + carry;
        *byte = (next & 0xff) as u8;
        carry = next >> 8;
    }
    if carry != 0 {
        return Err("uint256 value overflow".to_string());
    }
    Ok(())
}

pub(crate) fn add_u256_small(value: &mut [u8; 32], add: u8) -> Result<(), String> {
    let mut carry = add as u16;
    for byte in value.iter_mut().rev() {
        let next = (*byte as u16) + carry;
        *byte = (next & 0xff) as u8;
        carry = next >> 8;
        if carry == 0 {
            return Ok(());
        }
    }
    Err("uint256 value overflow".to_string())
}
