//! Chunked-payload object identity: the non-media analogue of
//! [`crate::media::CencFmp4MediaIdentityV1`]. Any file that is not fMP4/CENC
//! media is sealed as a framed header followed by a fixed number of
//! independently-addressable framed chunks (EPC1), so it can be read back one
//! chunk at a time instead of requiring the whole plaintext to be staged.

use elastos_protected_content_contracts::{
    CanonicalContract, ContractError, EncryptedContentIdentityV1,
};

use crate::media::{validate_visible_ascii, MAX_MEDIA_DECLARATION_BYTES_V1};

const CHUNKED_PAYLOAD_OBJECT_IDENTITY_DOMAIN_V1: &str =
    "elastos.protected-content.chunked-payload-object-identity/v1";

/// Plaintext bytes carried by one object chunk before framing.
pub const MAX_OBJECT_PLAINTEXT_CHUNK_BYTES_V1: usize = 1_048_576;
/// Ciphertext bytes for one framed chunk: plaintext chunk + AEAD tag + framing
/// slack. Asserted against `elastos-protected-content-custody`'s `payload.rs`
/// framing constant in Task 9 (this crate cannot see that crate).
pub const MAX_OBJECT_FRAMED_CHUNK_BYTES_V1: usize = MAX_OBJECT_PLAINTEXT_CHUNK_BYTES_V1 + 16 + 8;
/// Byte length of the magic + length + header block at file offset 0 of a
/// framed object.
pub const MAX_OBJECT_FRAMED_HEADER_BYTES_V1: usize = 4 + 2 + 512;
/// Largest plaintext object this contract will describe. `dkms`
/// `MAX_OBJECT_BYTES` parity: exactly `MAX_OBJECT_CHUNKS_V1` full chunks.
pub const MAX_OBJECT_PLAINTEXT_BYTES_V1: u64 = 64 * 1024 * 1024;
/// Largest number of chunks a plaintext object can require.
pub const MAX_OBJECT_CHUNKS_V1: u32 = 64;
/// Bound for the canonical byte encoding of [`ChunkedPayloadObjectIdentityV1`]
/// when carried as a `CanonicalBlob`. The encoding itself is a few hundred
/// bytes (a nested `EncryptedContentIdentityV1` plus a short content type
/// string and two integers); this bound leaves generous headroom.
pub const MAX_CHUNKED_PAYLOAD_OBJECT_IDENTITY_BYTES_V1: usize = 4096;

/// Identity of a chunked-payload object: the encrypted framed file as a
/// whole, its declared content type, the plaintext size it decrypts to, and
/// the byte length of the framed header block at the start of the file.
///
/// Mirrors [`crate::media::CencFmp4MediaIdentityV1`]'s validation idiom,
/// error type, and accessor style for the non-media case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkedPayloadObjectIdentityV1 {
    encrypted_content: EncryptedContentIdentityV1,
    content_type: String,
    plaintext_bytes: u64,
    framed_header_bytes: u32,
}

impl ChunkedPayloadObjectIdentityV1 {
    pub fn new(
        encrypted_content: EncryptedContentIdentityV1,
        content_type: impl Into<String>,
        plaintext_bytes: u64,
        framed_header_bytes: u32,
    ) -> Result<Self, ContractError> {
        let value = Self {
            encrypted_content,
            content_type: content_type.into(),
            plaintext_bytes,
            framed_header_bytes,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn encrypted_content(&self) -> &EncryptedContentIdentityV1 {
        &self.encrypted_content
    }

    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    pub const fn plaintext_bytes(&self) -> u64 {
        self.plaintext_bytes
    }

    pub const fn framed_header_bytes(&self) -> u32 {
        self.framed_header_bytes
    }

    /// `ceil(plaintext_bytes / MAX_OBJECT_PLAINTEXT_CHUNK_BYTES_V1)`.
    pub fn chunk_count(&self) -> u32 {
        let chunk_bytes = MAX_OBJECT_PLAINTEXT_CHUNK_BYTES_V1 as u64;
        u32::try_from(self.plaintext_bytes.div_ceil(chunk_bytes)).expect(
            "plaintext_bytes is validated to be within MAX_OBJECT_PLAINTEXT_BYTES_V1, \
             which yields at most MAX_OBJECT_CHUNKS_V1 chunks",
        )
    }

    fn validate(&self) -> Result<(), ContractError> {
        self.encrypted_content
            .canonical_bytes()
            .map_err(|_| ContractError::InvalidField("encrypted_content"))?;
        validate_visible_ascii(
            &self.content_type,
            "content_type",
            MAX_MEDIA_DECLARATION_BYTES_V1,
        )?;
        if self.plaintext_bytes == 0 || self.plaintext_bytes > MAX_OBJECT_PLAINTEXT_BYTES_V1 {
            return Err(ContractError::InvalidField("plaintext_bytes"));
        }
        if self.framed_header_bytes == 0
            || (self.framed_header_bytes as usize) > MAX_OBJECT_FRAMED_HEADER_BYTES_V1
        {
            return Err(ContractError::InvalidField("framed_header_bytes"));
        }
        if u64::from(self.framed_header_bytes) > self.encrypted_content.ciphertext_bytes() {
            return Err(ContractError::InvalidField("framed_header_bytes"));
        }
        // Encryption never shrinks: the framed file must be at least as long
        // as the header it starts with plus the plaintext it decrypts to.
        let minimum_ciphertext_bytes = u64::from(self.framed_header_bytes)
            .checked_add(self.plaintext_bytes)
            .ok_or(ContractError::InvalidField("plaintext_bytes"))?;
        if self.encrypted_content.ciphertext_bytes() < minimum_ciphertext_bytes {
            return Err(ContractError::InvalidField("encrypted_content"));
        }
        Ok(())
    }
}

impl CanonicalContract for ChunkedPayloadObjectIdentityV1 {
    fn canonical_bytes(&self) -> Result<Vec<u8>, ContractError> {
        self.validate()?;
        let mut bytes = canonical_prefix(CHUNKED_PAYLOAD_OBJECT_IDENTITY_DOMAIN_V1);
        push_bytes(
            &mut bytes,
            "encrypted_content",
            &self.encrypted_content.canonical_bytes()?,
        )?;
        push_string(&mut bytes, &self.content_type)?;
        bytes.extend_from_slice(&self.plaintext_bytes.to_be_bytes());
        bytes.extend_from_slice(&self.framed_header_bytes.to_be_bytes());
        Ok(bytes)
    }

    fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ContractError> {
        let mut cursor = canonical_cursor(bytes, CHUNKED_PAYLOAD_OBJECT_IDENTITY_DOMAIN_V1)?;
        let encrypted_content = EncryptedContentIdentityV1::from_canonical_bytes(&read_bytes(
            bytes,
            &mut cursor,
            "encrypted_content",
            u16::MAX as usize,
        )?)?;
        let content_type = read_string(
            bytes,
            &mut cursor,
            "content_type",
            MAX_MEDIA_DECLARATION_BYTES_V1,
        )?;
        let plaintext_bytes = read_u64(bytes, &mut cursor)?;
        let framed_header_bytes = read_u32(bytes, &mut cursor)?;
        let value = Self::new(
            encrypted_content,
            content_type,
            plaintext_bytes,
            framed_header_bytes,
        )?;
        finish_canonical(bytes, cursor)?;
        if value.canonical_bytes()?.as_slice() != bytes {
            return Err(ContractError::InvalidField("non-canonical encoding"));
        }
        Ok(value)
    }
}

// The canonical byte helpers below intentionally duplicate
// `crate::media`'s private (module-scoped) helpers of the same name: that
// module's helpers are not `pub(crate)`, and `elastos-protected-content-contracts`'s
// own `Encoder`/`Decoder` machinery is `pub(crate)` to that other crate, so
// neither is reachable from here. Keeping the duplication file-local mirrors
// `media.rs`'s own idiom rather than inventing a new shared abstraction.

fn canonical_prefix(domain: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(domain.len() + 1);
    bytes.extend_from_slice(domain.as_bytes());
    bytes.push(0);
    bytes
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn push_bytes(bytes: &mut Vec<u8>, field: &'static str, value: &[u8]) -> Result<(), ContractError> {
    let length = u16::try_from(value.len()).map_err(|_| ContractError::FieldTooLong(field))?;
    push_u16(bytes, length);
    bytes.extend_from_slice(value);
    Ok(())
}

fn push_string(bytes: &mut Vec<u8>, value: &str) -> Result<(), ContractError> {
    push_bytes(bytes, "canonical string", value.as_bytes())
}

fn canonical_cursor(bytes: &[u8], domain: &str) -> Result<usize, ContractError> {
    let prefix = domain.as_bytes();
    if bytes.len() <= prefix.len() || &bytes[..prefix.len()] != prefix || bytes[prefix.len()] != 0 {
        return Err(ContractError::WrongDomain);
    }
    Ok(prefix.len() + 1)
}

fn take_bytes<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    length: usize,
) -> Result<&'a [u8], ContractError> {
    let end = cursor
        .checked_add(length)
        .ok_or(ContractError::UnexpectedEnd)?;
    let value = bytes
        .get(*cursor..end)
        .ok_or(ContractError::UnexpectedEnd)?;
    *cursor = end;
    Ok(value)
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, ContractError> {
    Ok(u16::from_be_bytes(
        take_bytes(bytes, cursor, 2)?
            .try_into()
            .map_err(|_| ContractError::UnexpectedEnd)?,
    ))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, ContractError> {
    Ok(u32::from_be_bytes(
        take_bytes(bytes, cursor, 4)?
            .try_into()
            .map_err(|_| ContractError::UnexpectedEnd)?,
    ))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, ContractError> {
    Ok(u64::from_be_bytes(
        take_bytes(bytes, cursor, 8)?
            .try_into()
            .map_err(|_| ContractError::UnexpectedEnd)?,
    ))
}

fn read_bytes(
    bytes: &[u8],
    cursor: &mut usize,
    field: &'static str,
    maximum: usize,
) -> Result<Vec<u8>, ContractError> {
    let length = usize::from(read_u16(bytes, cursor)?);
    if length > maximum {
        return Err(ContractError::FieldTooLong(field));
    }
    Ok(take_bytes(bytes, cursor, length)?.to_vec())
}

fn read_string(
    bytes: &[u8],
    cursor: &mut usize,
    field: &'static str,
    maximum: usize,
) -> Result<String, ContractError> {
    String::from_utf8(read_bytes(bytes, cursor, field, maximum)?)
        .map_err(|_| ContractError::InvalidUtf8(field))
}

fn finish_canonical(bytes: &[u8], cursor: usize) -> Result<(), ContractError> {
    if cursor == bytes.len() {
        Ok(())
    } else {
        Err(ContractError::TrailingBytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_protected_content_contracts::Digest32;

    fn encrypted_content(seed: u8, bytes: u64) -> EncryptedContentIdentityV1 {
        EncryptedContentIdentityV1::new(Digest32::new([seed; 32]), bytes).unwrap()
    }

    fn identity_with(plaintext_bytes: u64) -> ChunkedPayloadObjectIdentityV1 {
        ChunkedPayloadObjectIdentityV1::new(
            encrypted_content(0x11, plaintext_bytes + 64),
            "application/octet-stream",
            plaintext_bytes,
            64,
        )
        .unwrap()
    }

    #[test]
    fn canonical_bytes_are_stable_and_round_trip() {
        let identity = identity_with(4096);
        let first = identity.canonical_bytes().unwrap();
        let second = identity.canonical_bytes().unwrap();
        assert_eq!(first, second);

        let decoded = ChunkedPayloadObjectIdentityV1::from_canonical_bytes(&first).unwrap();
        assert_eq!(decoded, identity);
        assert_eq!(decoded.canonical_bytes().unwrap(), first);
    }

    #[test]
    fn from_canonical_bytes_rejects_trailing_bytes_and_wrong_domain() {
        let identity = identity_with(4096);
        let mut bytes = identity.canonical_bytes().unwrap();
        bytes.push(0);
        assert!(ChunkedPayloadObjectIdentityV1::from_canonical_bytes(&bytes).is_err());
        assert!(ChunkedPayloadObjectIdentityV1::from_canonical_bytes(b"not-canonical").is_err());
    }

    #[test]
    fn content_type_rejects_oversize_and_non_ascii() {
        let too_long = "a".repeat(MAX_MEDIA_DECLARATION_BYTES_V1 + 1);
        assert!(ChunkedPayloadObjectIdentityV1::new(
            encrypted_content(0x11, 4096 + 64),
            too_long,
            4096,
            64,
        )
        .is_err());
        assert!(ChunkedPayloadObjectIdentityV1::new(
            encrypted_content(0x11, 4096 + 64),
            "application/octet-stream\n",
            4096,
            64,
        )
        .is_err());

        let max_len = "a".repeat(MAX_MEDIA_DECLARATION_BYTES_V1);
        assert!(ChunkedPayloadObjectIdentityV1::new(
            encrypted_content(0x11, 4096 + 64),
            max_len,
            4096,
            64,
        )
        .is_ok());
    }

    #[test]
    fn plaintext_bytes_boundary_is_exact_at_max_object_bytes() {
        assert!(ChunkedPayloadObjectIdentityV1::new(
            encrypted_content(0x11, MAX_OBJECT_PLAINTEXT_BYTES_V1 + 64),
            "application/octet-stream",
            MAX_OBJECT_PLAINTEXT_BYTES_V1,
            64,
        )
        .is_ok());
        assert!(ChunkedPayloadObjectIdentityV1::new(
            encrypted_content(0x11, MAX_OBJECT_PLAINTEXT_BYTES_V1 + 65),
            "application/octet-stream",
            MAX_OBJECT_PLAINTEXT_BYTES_V1 + 1,
            64,
        )
        .is_err());
        assert!(ChunkedPayloadObjectIdentityV1::new(
            encrypted_content(0x11, 64),
            "application/octet-stream",
            0,
            64,
        )
        .is_err());
    }

    #[test]
    fn chunk_count_boundary_is_exact_at_one_mebibyte() {
        let at_boundary = identity_with(MAX_OBJECT_PLAINTEXT_CHUNK_BYTES_V1 as u64);
        assert_eq!(at_boundary.chunk_count(), 1);

        let one_past = identity_with(MAX_OBJECT_PLAINTEXT_CHUNK_BYTES_V1 as u64 + 1);
        assert_eq!(one_past.chunk_count(), 2);

        let max = identity_with(MAX_OBJECT_PLAINTEXT_BYTES_V1);
        assert_eq!(max.chunk_count(), MAX_OBJECT_CHUNKS_V1);
    }

    #[test]
    fn framed_header_bytes_must_be_bounded_and_fit_within_the_framed_file() {
        assert!(ChunkedPayloadObjectIdentityV1::new(
            encrypted_content(0x11, 4096 + 64),
            "application/octet-stream",
            4096,
            0,
        )
        .is_err());
        assert!(ChunkedPayloadObjectIdentityV1::new(
            encrypted_content(0x11, 4096 + 64),
            "application/octet-stream",
            4096,
            u32::try_from(MAX_OBJECT_FRAMED_HEADER_BYTES_V1).unwrap() + 1,
        )
        .is_err());
        assert!(ChunkedPayloadObjectIdentityV1::new(
            encrypted_content(0x11, 32),
            "application/octet-stream",
            4096,
            64,
        )
        .is_err());
    }

    #[test]
    fn max_object_chunks_times_chunk_bytes_equals_max_object_plaintext_bytes() {
        assert_eq!(
            u64::from(MAX_OBJECT_CHUNKS_V1) * MAX_OBJECT_PLAINTEXT_CHUNK_BYTES_V1 as u64,
            MAX_OBJECT_PLAINTEXT_BYTES_V1
        );
    }

    #[test]
    fn encrypted_content_must_be_at_least_header_plus_plaintext_bytes() {
        // Exactly at the minimum (no ciphertext expansion at all): allowed.
        assert!(ChunkedPayloadObjectIdentityV1::new(
            encrypted_content(0x11, 4096 + 64),
            "application/octet-stream",
            4096,
            64,
        )
        .is_ok());
        // One byte short of the minimum: rejected, even though every other
        // field individually passes its own bound check.
        assert!(ChunkedPayloadObjectIdentityV1::new(
            encrypted_content(0x11, 4096 + 64 - 1),
            "application/octet-stream",
            4096,
            64,
        )
        .is_err());
        // Comfortably larger than the minimum (real AEAD framing overhead):
        // allowed.
        assert!(ChunkedPayloadObjectIdentityV1::new(
            encrypted_content(0x11, 4096 + 64 + 1024),
            "application/octet-stream",
            4096,
            64,
        )
        .is_ok());
    }
}
