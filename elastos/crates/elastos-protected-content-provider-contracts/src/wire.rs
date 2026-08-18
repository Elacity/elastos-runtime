use std::{fmt, io};

use elastos_protected_content_contracts::{
    CanonicalContract, ContractError, MAX_KEY_ENVELOPE_BYTES,
    MAX_RECIPIENT_SEALED_CONTRIBUTION_BYTES, MAX_RIGHTS_POLICY_BYTES, MAX_THRESHOLD_NODES,
};
use serde::{
    de::{DeserializeOwned, Error as _, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};

pub const MAX_PROVIDER_BINDING_BYTES_V1: usize = 4096;
pub const MAX_PROVIDER_FRAME_BYTES_V1: usize = 16 * 1024 * 1024;
pub const MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1: usize = 32;
pub const MAX_RECIPIENT_IDENTITY_BYTES_V1: usize = 256;
pub const MAX_SIGNED_RUNTIME_RELEASE_OPERATION_BYTES_V1: usize =
    (MAX_RIGHTS_POLICY_BYTES as usize) + (32 * 1024);
pub const MAX_SIGNED_NODE_RIGHTS_DECISION_BYTES_V1: usize = 8 * 1024;
pub const MAX_CUSTODY_ENVELOPE_BYTES_V1: usize = MAX_KEY_ENVELOPE_BYTES as usize;
pub const MAX_CUSTODY_NODE_PROVISIONING_RECORD_BYTES_V1: usize = MAX_KEY_ENVELOPE_BYTES as usize;
pub const MAX_SIGNED_RUNTIME_CUSTODY_PROVISIONING_BYTES_V1: usize = 8 * 1024;
pub const MAX_SIGNED_NODE_CONTRIBUTION_BYTES_V1: usize =
    MAX_RECIPIENT_SEALED_CONTRIBUTION_BYTES + (8 * 1024);
pub const MAX_SIGNED_TERMINAL_RECEIPT_BYTES_V1: usize = 8 * 1024;
pub(crate) const MAX_SIGNED_NODE_CONTRIBUTIONS_COUNT_V1: usize = MAX_THRESHOLD_NODES as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailureCodeV1 {
    NotConfigured,
    InvalidRequest,
    BindingMismatch,
    HandleAbsent,
    BackendUnavailable,
    InternalFailure,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CanonicalBlob<const MAX: usize>(Vec<u8>);

impl<const MAX: usize> CanonicalBlob<MAX> {
    pub(crate) fn new(value: Vec<u8>) -> Result<Self, ContractError> {
        if value.is_empty() || value.len() > MAX {
            return Err(ContractError::FieldTooLong("provider_canonical_blob"));
        }
        Ok(Self(value))
    }

    pub(crate) fn from_contract<T: CanonicalContract>(value: &T) -> Result<Self, ContractError> {
        Self::new(value.canonical_bytes()?)
    }

    pub(crate) fn decode<T: CanonicalContract>(&self) -> Result<T, ContractError> {
        T::from_canonical_bytes(self.as_slice())
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl<const MAX: usize> fmt::Debug for CanonicalBlob<MAX> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalBlob")
            .field("len", &self.0.len())
            .field("bytes", &"[redacted]")
            .finish()
    }
}

impl<const MAX: usize> Serialize for CanonicalBlob<MAX> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de, const MAX: usize> Deserialize<'de> for CanonicalBlob<MAX> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_bytes(CanonicalBlobVisitor::<MAX>)
    }
}

struct CanonicalBlobVisitor<const MAX: usize>;

impl<'de, const MAX: usize> Visitor<'de> for CanonicalBlobVisitor<MAX> {
    type Value = CanonicalBlob<MAX>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "1..={MAX} canonical bytes")
    }

    fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        CanonicalBlob::new(value.to_vec()).map_err(E::custom)
    }

    fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        CanonicalBlob::new(value).map_err(E::custom)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        if let Some(hint) = seq.size_hint() {
            if hint > MAX {
                return Err(A::Error::custom("canonical blob exceeds provider bounds"));
            }
        }
        let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(MAX));
        while let Some(byte) = seq.next_element::<u8>()? {
            if bytes.len() == MAX {
                return Err(A::Error::custom("canonical blob exceeds provider bounds"));
            }
            bytes.push(byte);
        }
        CanonicalBlob::new(bytes).map_err(A::Error::custom)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CanonicalBlobList<const MAX_ITEMS: usize, const MAX_ITEM_BYTES: usize>(
    Vec<CanonicalBlob<MAX_ITEM_BYTES>>,
);

impl<const MAX_ITEMS: usize, const MAX_ITEM_BYTES: usize>
    CanonicalBlobList<MAX_ITEMS, MAX_ITEM_BYTES>
{
    pub(crate) fn new(items: Vec<CanonicalBlob<MAX_ITEM_BYTES>>) -> Result<Self, ContractError> {
        if items.is_empty() || items.len() > MAX_ITEMS {
            return Err(ContractError::FieldTooLong("provider_canonical_blob_list"));
        }
        Ok(Self(items))
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &[u8]> {
        self.0.iter().map(CanonicalBlob::as_slice)
    }
}

impl<const MAX_ITEMS: usize, const MAX_ITEM_BYTES: usize> fmt::Debug
    for CanonicalBlobList<MAX_ITEMS, MAX_ITEM_BYTES>
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalBlobList")
            .field("count", &self.0.len())
            .field("items", &"[redacted]")
            .finish()
    }
}

impl<const MAX_ITEMS: usize, const MAX_ITEM_BYTES: usize> Serialize
    for CanonicalBlobList<MAX_ITEMS, MAX_ITEM_BYTES>
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let raw: Vec<&[u8]> = self.0.iter().map(CanonicalBlob::as_slice).collect();
        raw.serialize(serializer)
    }
}

impl<'de, const MAX_ITEMS: usize, const MAX_ITEM_BYTES: usize> Deserialize<'de>
    for CanonicalBlobList<MAX_ITEMS, MAX_ITEM_BYTES>
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(CanonicalBlobListVisitor::<MAX_ITEMS, MAX_ITEM_BYTES>)
    }
}

struct CanonicalBlobListVisitor<const MAX_ITEMS: usize, const MAX_ITEM_BYTES: usize>;

impl<'de, const MAX_ITEMS: usize, const MAX_ITEM_BYTES: usize> Visitor<'de>
    for CanonicalBlobListVisitor<MAX_ITEMS, MAX_ITEM_BYTES>
{
    type Value = CanonicalBlobList<MAX_ITEMS, MAX_ITEM_BYTES>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "1..={MAX_ITEMS} canonical blobs, each 1..={MAX_ITEM_BYTES} bytes"
        )
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        if let Some(hint) = seq.size_hint() {
            if hint > MAX_ITEMS {
                return Err(A::Error::custom(
                    "canonical blob list exceeds provider bounds",
                ));
            }
        }
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(MAX_ITEMS));
        while let Some(item) = seq.next_element::<CanonicalBlob<MAX_ITEM_BYTES>>()? {
            if items.len() == MAX_ITEMS {
                return Err(A::Error::custom(
                    "canonical blob list exceeds provider bounds",
                ));
            }
            items.push(item);
        }
        CanonicalBlobList::new(items).map_err(A::Error::custom)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct OpaqueHandleV1([u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]);

impl OpaqueHandleV1 {
    pub fn new(value: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]) -> Result<Self, ContractError> {
        validate_opaque_handle_bytes(&value)?;
        Ok(Self(value))
    }

    pub const fn as_bytes(&self) -> &[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1] {
        &self.0
    }
}

impl fmt::Debug for OpaqueHandleV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("\"[redacted]\"")
    }
}

impl Serialize for OpaqueHandleV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&encode_lower_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for OpaqueHandleV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let bytes = decode_lower_hex_handle(&value).map_err(D::Error::custom)?;
        Ok(Self(bytes))
    }
}

pub(crate) fn decode_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_json::Error> {
    if bytes.is_empty() {
        return Err(provider_frame_error("provider frame is empty"));
    }
    if bytes.len() > MAX_PROVIDER_FRAME_BYTES_V1 {
        return Err(provider_frame_error("provider frame exceeds bounds"));
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = T::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}

pub(crate) fn contract_decode_error(error: ContractError) -> serde_json::Error {
    serde_json::Error::io(io::Error::new(
        io::ErrorKind::InvalidData,
        error.to_string(),
    ))
}

pub(crate) fn encode_json<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.is_empty() {
        return Err(provider_frame_error("provider frame is empty"));
    }
    if bytes.len() > MAX_PROVIDER_FRAME_BYTES_V1 {
        return Err(provider_frame_error("provider frame exceeds bounds"));
    }
    Ok(bytes)
}

pub(crate) fn validate_schema(
    actual: &str,
    expected: &'static str,
    field: &'static str,
) -> Result<(), ContractError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ContractError::InvalidField(field))
    }
}

pub(crate) fn validate_time_window(
    issued_at: u64,
    expires_at: u64,
    max_lifetime_secs: u64,
    field: &'static str,
) -> Result<(), ContractError> {
    if issued_at >= expires_at {
        return Err(ContractError::InvalidField(field));
    }
    if expires_at - issued_at > max_lifetime_secs {
        return Err(ContractError::InvalidField(field));
    }
    Ok(())
}

fn validate_opaque_handle_bytes(
    value: &[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
) -> Result<(), ContractError> {
    if value.iter().all(|byte| *byte == 0) {
        return Err(ContractError::InvalidField("opaque_handle"));
    }
    Ok(())
}

fn provider_frame_error(message: &'static str) -> serde_json::Error {
    serde_json::Error::io(io::Error::new(io::ErrorKind::InvalidData, message))
}

fn encode_lower_hex(bytes: &[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]) -> String {
    let mut text = String::with_capacity(MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1 * 2);
    for byte in bytes {
        text.push(nibble_to_hex(byte >> 4));
        text.push(nibble_to_hex(byte & 0x0f));
    }
    text
}

fn decode_lower_hex_handle(
    value: &str,
) -> Result<[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1], ContractError> {
    if value.len() != MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1 * 2 {
        return Err(ContractError::InvalidField("opaque_handle"));
    }
    let mut bytes = [0u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_lower_hex_nibble(chunk[0])?;
        let low = decode_lower_hex_nibble(chunk[1])?;
        bytes[index] = (high << 4) | low;
    }
    validate_opaque_handle_bytes(&bytes)?;
    Ok(bytes)
}

fn nibble_to_hex(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        10..=15 => char::from(b'a' + (value - 10)),
        _ => unreachable!("nibble must be <= 15"),
    }
}

fn decode_lower_hex_nibble(value: u8) -> Result<u8, ContractError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(10 + value - b'a'),
        _ => Err(ContractError::InvalidField("opaque_handle")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        decode_json, encode_json, OpaqueHandleV1, MAX_PROVIDER_FRAME_BYTES_V1,
        MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1,
    };

    #[test]
    fn opaque_handle_rejects_zero_and_redacts_debug() {
        assert!(OpaqueHandleV1::new([0u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]).is_err());

        let mut bytes = [0u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
        bytes[0] = 1;
        let handle = OpaqueHandleV1::new(bytes).unwrap();
        let debug = format!("{handle:?}");

        assert_eq!(handle.as_bytes(), &bytes);
        assert_eq!(debug, "\"[redacted]\"");
        assert!(!debug.contains("01"));
    }

    #[test]
    fn opaque_handle_rejects_topology_text_and_changes_on_mutation() {
        for invalid in [
            "\"/private/tmp/provider.sock\"",
            "\"https://node.example:7443/path\"",
            "\"host.example:9000\"",
            "\"ticket:abc123\"",
            "\"credential=secret\"",
        ] {
            assert!(serde_json::from_str::<OpaqueHandleV1>(invalid).is_err());
        }

        let mut bytes = [0u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
        bytes[0] = 1;
        let original = OpaqueHandleV1::new(bytes).unwrap();
        bytes[31] = 2;
        let mutated = OpaqueHandleV1::new(bytes).unwrap();
        assert_ne!(original, mutated);
    }

    #[test]
    fn opaque_handle_requires_exact_lowercase_hex_text() {
        let mut bytes = [0u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
        bytes[0] = 0x12;
        bytes[31] = 0xab;
        let handle = OpaqueHandleV1::new(bytes).unwrap();
        let encoded = serde_json::to_string(&handle).unwrap();

        assert_eq!(encoded.len(), (MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1 * 2) + 2);
        assert_eq!(
            serde_json::from_str::<OpaqueHandleV1>(&encoded).unwrap(),
            handle
        );
        assert!(serde_json::from_str::<OpaqueHandleV1>(
            "\"12000000000000000000000000000000000000000000000000000000000000AB\""
        )
        .is_err());
    }

    #[test]
    fn provider_frame_accepts_exact_limit_and_rejects_limit_plus_one() {
        let exact = "a".repeat(MAX_PROVIDER_FRAME_BYTES_V1 - 2);
        let encoded = encode_json(&exact).unwrap();
        assert_eq!(encoded.len(), MAX_PROVIDER_FRAME_BYTES_V1);
        assert_eq!(decode_json::<String>(&encoded).unwrap(), exact);

        let over = "a".repeat(MAX_PROVIDER_FRAME_BYTES_V1 - 1);
        assert!(encode_json(&over).is_err());
        assert!(
            decode_json::<serde_json::Value>(&vec![b' '; MAX_PROVIDER_FRAME_BYTES_V1 + 1]).is_err()
        );
        assert!(decode_json::<serde_json::Value>(b"").is_err());
    }
}
