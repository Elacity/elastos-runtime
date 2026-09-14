//! MPEG Common Encryption (CENC, ISO/IEC 23001-7 §8.1) Protection System Specific
//! Header (`pssh`) construction, parsing, and payload schema for the Elacity
//! PQ-hybrid-threshold protection scheme.
//!
//! This module is the single place the `pssh` wire form is written or read. The
//! producer that protects a clear fMP4 init segment, the read path that rewrites a
//! protected init segment back to clear, and the layout validator that admits the
//! box all call the same `build_elastos_pq_pssh_v1` / `parse_pssh_v1` /
//! [`ElastosPqPsshDataV1`] here, so the three cannot drift apart.
//!
//! The box `Data` payload is JSON describing the *scheme*, never key material: it
//! ships inside a public media file. It carries the content access id a consumer
//! presents when it asks for a key, and the custody pool, epoch and committee
//! authorization identities that say which quorum can answer. Nothing in it is
//! secret, and nothing in it can be used to decrypt without a separate authorized
//! key release.

use elastos_protected_content_contracts::{
    ContentAccessIdV1, ContractError, CustodyCommitteeAuthorizationIdentityV1,
    CustodyEpochIdentityV1, CustodyPoolIdentityV1, Digest32, CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
};
use serde::{Deserialize, Serialize};

use crate::media::CENC_FMP4_MEDIA_SUITE_ID_V1;

/// Registered DRM System ID for `cenc:elastos-pq-hybrid-threshold-v0`.
/// UUID `b6e254ef-0dc5-47fe-94e7-0e72ed1dc7b0` · PSSH base64 `tuJU7w3FR/6U5w5y7R3HsA==`.
pub const ELASTOS_PQ_SYSTEM_ID: [u8; 16] = [
    0xb6, 0xe2, 0x54, 0xef, 0x0d, 0xc5, 0x47, 0xfe, 0x94, 0xe7, 0x0e, 0x72, 0xed, 0x1d, 0xc7, 0xb0,
];

/// The versioned shape of the JSON carried in the box's `Data` field.
pub const ELASTOS_PQ_PSSH_DATA_SCHEMA_V1: &str = "elastos.protected-content.cenc-pssh-data/v1";

/// The protection scheme the system id is registered for.
pub const ELASTOS_PQ_PROTECTION_SCHEME_V1: &str = "cenc:elastos-pq-hybrid-threshold-v0";

/// A `Data` payload larger than this is refused rather than parsed: the payload is
/// a fixed set of identity fields, so anything near this size is malformed.
pub const MAX_ELASTOS_PQ_PSSH_DATA_BYTES_V1: usize = 4096;

/// A parsed **version-1** `pssh` box: system id, embedded KID list, and the opaque
/// key-acquisition payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsshBoxV1 {
    pub system_id: [u8; 16],
    pub kids: Vec<[u8; 16]>,
    pub data: Vec<u8>,
}

/// The JSON `Data` payload: everything a consumer needs to recognise the scheme and
/// start a key acquisition, and nothing else.
///
/// Field order here is the serialized order, so the same inputs always produce the
/// same bytes and therefore the same init segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElastosPqPsshDataV1 {
    /// Always [`ELASTOS_PQ_PSSH_DATA_SCHEMA_V1`].
    pub schema: String,
    /// Always [`ELASTOS_PQ_PROTECTION_SCHEME_V1`] — names the whole scheme the
    /// system id stands for, so a consumer that recognises only the UUID and this
    /// string knows what it is looking at.
    pub protection_scheme: String,
    /// The sample-encryption profile of the media itself
    /// ([`CENC_FMP4_MEDIA_SUITE_ID_V1`]): AES-128-CTR full-sample CENC over fMP4,
    /// 8-byte IVs. Tells a consumer how to apply the key once it has one.
    pub content_encryption: String,
    /// The key-encapsulation suite the released content key arrives under
    /// ([`CUSTODY_X_WING_AES256GCM_SUITE_ID_V1`]): X-Wing draft-06 — ML-KEM-768
    /// concatenated with X25519 — HKDF-SHA256, AES-256-GCM. Tells a consumer which
    /// session key type to mint before it asks.
    pub key_encapsulation: String,
    /// Lowercase hex of the 16-byte content access id. This is the `tenc` default
    /// KID and the box's KID-list entry, and it is the handle a consumer presents
    /// when it asks custody for the content key.
    pub content_access_id: String,
    /// Lowercase hex of the custody pool identity digest, and its canonical byte
    /// length. Says which custody pool holds the shares.
    pub custody_pool_sha256: String,
    pub custody_pool_bytes: u32,
    /// Lowercase hex of the custody epoch identity digest, and its canonical byte
    /// length. Says which epoch's node set and thresholds apply.
    pub custody_epoch_sha256: String,
    pub custody_epoch_bytes: u32,
    /// Lowercase hex of the custody committee authorization identity digest, and
    /// its canonical byte length. Says which committee authorization a release must
    /// be evaluated under.
    pub custody_committee_authorization_sha256: String,
    pub custody_committee_authorization_bytes: u32,
}

impl ElastosPqPsshDataV1 {
    pub fn new(
        content_access_id: ContentAccessIdV1,
        custody_pool: &CustodyPoolIdentityV1,
        custody_epoch: &CustodyEpochIdentityV1,
        custody_committee_authorization: &CustodyCommitteeAuthorizationIdentityV1,
    ) -> Self {
        Self {
            schema: ELASTOS_PQ_PSSH_DATA_SCHEMA_V1.to_string(),
            protection_scheme: ELASTOS_PQ_PROTECTION_SCHEME_V1.to_string(),
            content_encryption: CENC_FMP4_MEDIA_SUITE_ID_V1.to_string(),
            key_encapsulation: CUSTODY_X_WING_AES256GCM_SUITE_ID_V1.to_string(),
            content_access_id: hex_lower(content_access_id.as_bytes()),
            custody_pool_sha256: hex_lower(custody_pool.pool_sha256().as_bytes()),
            custody_pool_bytes: custody_pool.pool_bytes(),
            custody_epoch_sha256: hex_lower(custody_epoch.epoch_sha256().as_bytes()),
            custody_epoch_bytes: custody_epoch.epoch_bytes(),
            custody_committee_authorization_sha256: hex_lower(
                custody_committee_authorization
                    .authorization_sha256()
                    .as_bytes(),
            ),
            custody_committee_authorization_bytes: custody_committee_authorization
                .authorization_bytes(),
        }
    }

    /// The 16-byte content access id this payload declares, re-parsed from its hex
    /// so a malformed or zero id fails here rather than downstream.
    pub fn parsed_content_access_id(&self) -> Result<ContentAccessIdV1, ContractError> {
        let bytes: [u8; 16] = parse_hex_lower::<16>(&self.content_access_id, "content_access_id")?;
        ContentAccessIdV1::new(bytes)
    }

    /// Rejects a payload whose constant fields are not this scheme's, or whose
    /// variable fields are not well-formed. Called on every parse.
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema != ELASTOS_PQ_PSSH_DATA_SCHEMA_V1 {
            return Err(ContractError::InvalidField("pssh_data_schema"));
        }
        if self.protection_scheme != ELASTOS_PQ_PROTECTION_SCHEME_V1 {
            return Err(ContractError::InvalidField("pssh_protection_scheme"));
        }
        if self.content_encryption != CENC_FMP4_MEDIA_SUITE_ID_V1 {
            return Err(ContractError::InvalidField("pssh_content_encryption"));
        }
        if self.key_encapsulation != CUSTODY_X_WING_AES256GCM_SUITE_ID_V1 {
            return Err(ContractError::InvalidField("pssh_key_encapsulation"));
        }
        let _ = self.parsed_content_access_id()?;
        let pool = Digest32::new(parse_hex_lower::<32>(
            &self.custody_pool_sha256,
            "pssh_custody_pool_sha256",
        )?);
        let epoch = Digest32::new(parse_hex_lower::<32>(
            &self.custody_epoch_sha256,
            "pssh_custody_epoch_sha256",
        )?);
        let authorization = Digest32::new(parse_hex_lower::<32>(
            &self.custody_committee_authorization_sha256,
            "pssh_custody_committee_authorization_sha256",
        )?);
        // Re-minting each identity applies the same field bounds the canonical
        // custody contracts apply, so a payload can never name an identity the
        // rest of the system would reject.
        let _ = CustodyPoolIdentityV1::new(pool, self.custody_pool_bytes)?;
        let _ = CustodyEpochIdentityV1::new(epoch, self.custody_epoch_bytes)?;
        let _ = CustodyCommitteeAuthorizationIdentityV1::new(
            authorization,
            self.custody_committee_authorization_bytes,
        )?;
        Ok(())
    }

    pub fn to_json_bytes(&self) -> Result<Vec<u8>, ContractError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|_| ContractError::InvalidField("pssh_data_payload"))?;
        if bytes.len() > MAX_ELASTOS_PQ_PSSH_DATA_BYTES_V1 {
            return Err(ContractError::FieldTooLong("pssh_data_payload"));
        }
        Ok(bytes)
    }

    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, ContractError> {
        if bytes.len() > MAX_ELASTOS_PQ_PSSH_DATA_BYTES_V1 {
            return Err(ContractError::FieldTooLong("pssh_data_payload"));
        }
        let value = serde_json::from_slice::<Self>(bytes)
            .map_err(|_| ContractError::InvalidField("pssh_data_payload"))?;
        value.validate()?;
        Ok(value)
    }
}

/// Build the **version-1** `pssh` box (ISO/IEC 23001-7 §8.1) that carries `data`
/// for `system_id`, listing `kids` as the embedded default-KID set.
///
/// Layout: `size:u32be ‖ "pssh" ‖ version=1:u8 ‖ flags:[0;3] ‖ system_id:16 ‖
/// kid_count:u32be ‖ kids:(16·n) ‖ data_size:u32be ‖ data`.
pub fn build_pssh_v1(system_id: &[u8; 16], kids: &[[u8; 16]], data: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.push(1u8); // version
    body.extend_from_slice(&[0u8, 0, 0]); // flags
    body.extend_from_slice(system_id);
    body.extend_from_slice(&(kids.len() as u32).to_be_bytes());
    for kid in kids {
        body.extend_from_slice(kid);
    }
    body.extend_from_slice(&(data.len() as u32).to_be_bytes());
    body.extend_from_slice(data);

    let total = 8 + body.len(); // 4 (size) + 4 ("pssh")
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(b"pssh");
    out.extend_from_slice(&body);
    out
}

/// Build the Elacity PQ-hybrid `pssh` box for `data`, with the declared content
/// access id as the single KID.
pub fn build_elastos_pq_pssh_v1(data: &ElastosPqPsshDataV1) -> Result<Vec<u8>, ContractError> {
    let kid = *data.parsed_content_access_id()?.as_bytes();
    Ok(build_pssh_v1(
        &ELASTOS_PQ_SYSTEM_ID,
        &[kid],
        &data.to_json_bytes()?,
    ))
}

/// Parse a single **version-1** `pssh` box. Returns `None` on anything malformed.
///
/// Tolerant of trailing bytes AFTER the box (e.g. when handed a slice of a larger
/// `moov`): the declared `size` bounds the read. Intolerant of everything else —
/// a box whose declared size runs past the buffer, whose version is not 1 (the
/// only version this scheme emits), or that leaves slack bytes between the end of
/// `Data` and the end of the box is malformed, not ours.
pub fn parse_pssh_v1(bytes: &[u8]) -> Option<PsshBoxV1> {
    if bytes.len() < 12 || &bytes[4..8] != b"pssh" {
        return None;
    }
    let size = u32::from_be_bytes(bytes[0..4].try_into().ok()?) as usize;
    if size < 12 || size > bytes.len() {
        return None;
    }
    let buf = &bytes[..size];
    if *buf.get(8)? != 1 {
        return None;
    }
    // buf[9..12] = flags (ignored)
    let mut off = 12usize;
    let system_id: [u8; 16] = buf.get(off..off + 16)?.try_into().ok()?;
    off += 16;
    let kid_count = u32::from_be_bytes(buf.get(off..off + 4)?.try_into().ok()?) as usize;
    off += 4;
    let mut kids = Vec::with_capacity(kid_count.min(16));
    for _ in 0..kid_count {
        let kid: [u8; 16] = buf.get(off..off + 16)?.try_into().ok()?;
        off += 16;
        kids.push(kid);
    }
    let data_len = u32::from_be_bytes(buf.get(off..off + 4)?.try_into().ok()?) as usize;
    off += 4;
    let data = buf.get(off..off + data_len)?.to_vec();
    if off.checked_add(data_len)? != size {
        return None;
    }
    Some(PsshBoxV1 {
        system_id,
        kids,
        data,
    })
}

/// Parse a `pssh` box that must be this scheme's: the registered system id, a
/// single KID, and a `Data` payload that is a well-formed [`ElastosPqPsshDataV1`]
/// whose declared content access id is that KID. Anything else is an error, so a
/// foreign system id or a malformed box can never pass as ours.
pub fn parse_elastos_pq_pssh_v1(bytes: &[u8]) -> Result<ElastosPqPsshDataV1, ContractError> {
    let parsed = parse_pssh_v1(bytes).ok_or(ContractError::InvalidField("pssh_box"))?;
    if parsed.system_id != ELASTOS_PQ_SYSTEM_ID {
        return Err(ContractError::InvalidField("pssh_system_id"));
    }
    let data = ElastosPqPsshDataV1::from_json_bytes(&parsed.data)?;
    let declared = *data.parsed_content_access_id()?.as_bytes();
    if parsed.kids.as_slice() != [declared] {
        return Err(ContractError::InvalidField("pssh_kids"));
    }
    Ok(data)
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

fn parse_hex_lower<const N: usize>(
    text: &str,
    field: &'static str,
) -> Result<[u8; N], ContractError> {
    if text.len() != N * 2 {
        return Err(ContractError::InvalidField(field));
    }
    let mut out = [0u8; N];
    let raw = text.as_bytes();
    for (index, slot) in out.iter_mut().enumerate() {
        let high = hex_nibble(raw[index * 2], field)?;
        let low = hex_nibble(raw[index * 2 + 1], field)?;
        *slot = (high << 4) | low;
    }
    Ok(out)
}

fn hex_nibble(byte: u8, field: &'static str) -> Result<u8, ContractError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        // Lowercase only: one spelling on the wire means one byte sequence for the
        // same payload, so the init segment stays reproducible.
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(ContractError::InvalidField(field)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> ElastosPqPsshDataV1 {
        ElastosPqPsshDataV1::new(
            ContentAccessIdV1::new([0x55; 16]).unwrap(),
            &CustodyPoolIdentityV1::new(Digest32::new([0x61; 32]), 512).unwrap(),
            &CustodyEpochIdentityV1::new(Digest32::new([0x62; 32]), 512).unwrap(),
            &CustodyCommitteeAuthorizationIdentityV1::new(Digest32::new([0x63; 32]), 512).unwrap(),
        )
    }

    #[test]
    fn system_id_is_the_assigned_uuid() {
        // b6e254ef-0dc5-47fe-94e7-0e72ed1dc7b0 (PSSH base64 tuJU7w3FR/6U5w5y7R3HsA==).
        assert_eq!(
            ELASTOS_PQ_SYSTEM_ID,
            [
                0xb6, 0xe2, 0x54, 0xef, 0x0d, 0xc5, 0x47, 0xfe, 0x94, 0xe7, 0x0e, 0x72, 0xed, 0x1d,
                0xc7, 0xb0
            ]
        );
    }

    #[test]
    fn box_grammar_matches_iso_23001_7_version_1() {
        let boxed = build_elastos_pq_pssh_v1(&data()).unwrap();
        assert_eq!(
            u32::from_be_bytes(boxed[0..4].try_into().unwrap()) as usize,
            boxed.len()
        );
        assert_eq!(&boxed[4..8], b"pssh");
        assert_eq!(boxed[8], 1, "version");
        assert_eq!(&boxed[9..12], &[0, 0, 0], "flags");
        assert_eq!(&boxed[12..28], &ELASTOS_PQ_SYSTEM_ID);
        assert_eq!(
            u32::from_be_bytes(boxed[28..32].try_into().unwrap()),
            1,
            "kid_count"
        );
        assert_eq!(&boxed[32..48], &[0x55u8; 16], "the default KID");
        let data_len = u32::from_be_bytes(boxed[48..52].try_into().unwrap()) as usize;
        assert_eq!(48 + 4 + data_len, boxed.len());
    }

    #[test]
    fn build_then_parse_round_trips_and_the_payload_carries_no_secret() {
        let value = data();
        let boxed = build_elastos_pq_pssh_v1(&value).unwrap();
        let parsed = parse_pssh_v1(&boxed).expect("parse");
        assert_eq!(parsed.system_id, ELASTOS_PQ_SYSTEM_ID);
        assert_eq!(parsed.kids, vec![[0x55u8; 16]]);
        assert_eq!(parse_elastos_pq_pssh_v1(&boxed).unwrap(), value);

        // Every field is either a constant scheme name or a public identity
        // digest, so the JSON is exactly the declared field set and nothing else.
        let json = String::from_utf8(value.to_json_bytes().unwrap()).unwrap();
        assert_eq!(
            json,
            concat!(
                r#"{"schema":"elastos.protected-content.cenc-pssh-data/v1","#,
                r#""protection_scheme":"cenc:elastos-pq-hybrid-threshold-v0","#,
                r#""content_encryption":"cenc-fmp4-aes128ctr/v1","#,
                r#""key_encapsulation":"elastos-xwing-draft06-hkdf-sha256-aes256gcm/v1","#,
                r#""content_access_id":"55555555555555555555555555555555","#,
                r#""custody_pool_sha256":"6161616161616161616161616161616161616161616161616161616161616161","#,
                r#""custody_pool_bytes":512,"#,
                r#""custody_epoch_sha256":"6262626262626262626262626262626262626262626262626262626262626262","#,
                r#""custody_epoch_bytes":512,"#,
                r#""custody_committee_authorization_sha256":"6363636363636363636363636363636363636363636363636363636363636363","#,
                r#""custody_committee_authorization_bytes":512}"#,
            )
        );
    }

    #[test]
    fn the_same_inputs_always_produce_the_same_bytes() {
        assert_eq!(
            build_elastos_pq_pssh_v1(&data()).unwrap(),
            build_elastos_pq_pssh_v1(&data()).unwrap()
        );
    }

    #[test]
    fn parse_tolerates_trailing_bytes_but_rejects_short_and_wrong_type() {
        let mut boxed = build_elastos_pq_pssh_v1(&data()).unwrap();
        boxed.extend_from_slice(b"TRAILING-MOOV-BYTES");
        assert_eq!(parse_elastos_pq_pssh_v1(&boxed).unwrap(), data());

        assert!(parse_pssh_v1(b"short").is_none());
        let mut wrong_type = build_elastos_pq_pssh_v1(&data()).unwrap();
        wrong_type[4] = b'm';
        assert!(parse_pssh_v1(&wrong_type).is_none());
        assert!(parse_elastos_pq_pssh_v1(&wrong_type).is_err());
    }

    #[test]
    fn parse_rejects_other_versions_oversized_declarations_and_slack_after_data() {
        // Version 0 is a legal CENC box but not one this scheme ever emits, and it
        // carries no KID list to bind. Anything else is simply malformed.
        for version in [0u8, 2, 255] {
            let mut other = build_elastos_pq_pssh_v1(&data()).unwrap();
            other[8] = version;
            assert!(parse_pssh_v1(&other).is_none(), "version {version}");
            assert!(
                parse_elastos_pq_pssh_v1(&other).is_err(),
                "version {version}"
            );
        }

        // A declared size past the end of the buffer is refused instead of being
        // silently truncated to what happens to be there.
        let mut oversized = build_elastos_pq_pssh_v1(&data()).unwrap();
        let grown = (oversized.len() as u32 + 8).to_be_bytes();
        oversized[0..4].copy_from_slice(&grown);
        assert!(parse_pssh_v1(&oversized).is_none());

        // Slack between the end of `Data` and the end of the box: the box claims
        // eight more bytes than its fields account for.
        let mut slack = build_elastos_pq_pssh_v1(&data()).unwrap();
        let grown = (slack.len() as u32 + 8).to_be_bytes();
        slack[0..4].copy_from_slice(&grown);
        slack.extend_from_slice(&[0u8; 8]);
        assert!(parse_pssh_v1(&slack).is_none());
        assert!(parse_elastos_pq_pssh_v1(&slack).is_err());

        // A zero size ("to end of file") is not a shape this scheme emits either.
        let mut zero_size = build_elastos_pq_pssh_v1(&data()).unwrap();
        zero_size[0..4].copy_from_slice(&0u32.to_be_bytes());
        assert!(parse_pssh_v1(&zero_size).is_none());
    }

    #[test]
    fn a_foreign_system_id_is_never_read_as_ours() {
        let mut foreign = ELASTOS_PQ_SYSTEM_ID;
        foreign[15] ^= 0x01;
        let boxed = build_pssh_v1(&foreign, &[[0x55; 16]], &data().to_json_bytes().unwrap());
        assert!(parse_pssh_v1(&boxed).is_some(), "still a valid pssh box");
        assert!(parse_elastos_pq_pssh_v1(&boxed).is_err());
    }

    #[test]
    fn payload_validation_pins_every_constant_and_rejects_bad_hex() {
        for mutate in [
            (|value: &mut ElastosPqPsshDataV1| value.schema = "other/v1".to_string())
                as fn(&mut ElastosPqPsshDataV1),
            |value| value.protection_scheme = "cenc:widevine".to_string(),
            |value| value.content_encryption = "cbcs".to_string(),
            |value| value.key_encapsulation = "rsa-2048".to_string(),
            |value| value.content_access_id = "00000000000000000000000000000000".to_string(),
            |value| value.content_access_id = "ZZ".to_string(),
            |value| value.custody_pool_sha256 = "6161".to_string(),
            // Uppercase hex is refused so one payload has one byte spelling.
            |value| value.custody_epoch_sha256 = "AB".repeat(32),
        ] {
            let mut value = data();
            mutate(&mut value);
            assert!(value.validate().is_err());
            assert!(
                ElastosPqPsshDataV1::from_json_bytes(&serde_json::to_vec(&value).unwrap()).is_err()
            );
        }
    }

    #[test]
    fn unknown_and_missing_json_fields_are_refused() {
        assert!(ElastosPqPsshDataV1::from_json_bytes(br#"{"schema":"x"}"#).is_err());
        let mut json = String::from_utf8(data().to_json_bytes().unwrap()).unwrap();
        json.insert_str(1, r#""extra":1,"#);
        assert!(ElastosPqPsshDataV1::from_json_bytes(json.as_bytes()).is_err());
        assert!(ElastosPqPsshDataV1::from_json_bytes(&vec![
            b'x';
            MAX_ELASTOS_PQ_PSSH_DATA_BYTES_V1 + 1
        ])
        .is_err());
    }
}
