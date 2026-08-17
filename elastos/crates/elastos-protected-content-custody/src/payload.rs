use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use rand09::{rngs::StdRng, RngCore as _, SeedableRng as _};
use sha2::{Digest as _, Sha256};
use std::io::{Read, Write};
use zeroize::Zeroizing;

use elastos_protected_content_contracts::{
    CustodyEnvelopeV1, CustodyEpochIdentityV1, Digest32, EncryptedContentIdentityV1,
    NodeCustodyPublicKeyV1, NodePublicKey, ThresholdV1, MAX_ENCRYPTED_CONTENT_BYTES,
};

use crate::{ContentEncryptionKeyV1, CustodyError};

const PAYLOAD_MAGIC_V1: [u8; 4] = *b"EPC1";
const PAYLOAD_HEADER_LENGTH_BYTES_V1: usize = 2;
const PAYLOAD_BASE_NONCE_BYTES_V1: usize = 12;
const PAYLOAD_TAG_BYTES_V1: usize = 16;
const PAYLOAD_SCHEMA_V1: &str = "elastos.protected-content.payload/v1";
const PAYLOAD_SUITE_ID_V1: &str = "aes-256-gcm-chunked/v1";
const PAYLOAD_CHUNK_AAD_DOMAIN_V1: &[u8] = b"elastos.protected-content.payload.chunk-aad/v1";

pub const PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1: u32 = 1_048_576;
pub const MAX_PAYLOAD_CONTENT_TYPE_BYTES_V1: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedChunkPayloadHeaderV1 {
    content_type: String,
    plaintext_bytes: u64,
    base_nonce: [u8; PAYLOAD_BASE_NONCE_BYTES_V1],
    content_key_commitment: Digest32,
}

impl AuthenticatedChunkPayloadHeaderV1 {
    fn new_authenticated(
        content_type: impl Into<String>,
        plaintext_bytes: u64,
        base_nonce: [u8; PAYLOAD_BASE_NONCE_BYTES_V1],
        content_key_commitment: Digest32,
    ) -> Result<Self, CustodyError> {
        let value = Self {
            content_type: content_type.into(),
            plaintext_bytes,
            base_nonce,
            content_key_commitment,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn schema(&self) -> &'static str {
        PAYLOAD_SCHEMA_V1
    }

    pub const fn suite_id(&self) -> &'static str {
        PAYLOAD_SUITE_ID_V1
    }

    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    pub const fn plaintext_bytes(&self) -> u64 {
        self.plaintext_bytes
    }

    fn base_nonce(&self) -> &[u8; PAYLOAD_BASE_NONCE_BYTES_V1] {
        &self.base_nonce
    }

    fn validate(&self) -> Result<(), CustodyError> {
        self.validate_basic_fields()?;
        if self.expected_framed_bytes()? > MAX_ENCRYPTED_CONTENT_BYTES {
            return Err(CustodyError::InvalidPayload("framed_ciphertext_bytes"));
        }
        Ok(())
    }

    fn validate_basic_fields(&self) -> Result<(), CustodyError> {
        if self.content_type.is_empty()
            || self.content_type.len() > MAX_PAYLOAD_CONTENT_TYPE_BYTES_V1
            || !self
                .content_type
                .as_bytes()
                .iter()
                .all(|byte| matches!(*byte, 0x21..=0x7e))
        {
            return Err(CustodyError::InvalidPayload("content_type"));
        }
        if self.plaintext_bytes == 0 {
            return Err(CustodyError::InvalidPayload("plaintext_bytes"));
        }
        Ok(())
    }

    fn chunk_count(&self) -> Result<u64, CustodyError> {
        self.plaintext_bytes
            .checked_add(u64::from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1 - 1))
            .ok_or(CustodyError::InvalidPayload("chunk_count"))?
            .checked_div(u64::from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1))
            .ok_or(CustodyError::InvalidPayload("chunk_count"))
    }

    fn encoded_len(&self) -> usize {
        2 + self.schema().len()
            + 2
            + self.suite_id().len()
            + 2
            + self.content_type.len()
            + 8
            + PAYLOAD_BASE_NONCE_BYTES_V1
            + 32
    }

    fn encoded_bytes(&self) -> Result<Vec<u8>, CustodyError> {
        self.validate_basic_fields()?;
        let schema = self.schema().as_bytes();
        let suite = self.suite_id().as_bytes();
        let content_type = self.content_type.as_bytes();
        let mut encoded = Vec::with_capacity(self.encoded_len());
        write_len_prefixed(&mut encoded, schema)?;
        write_len_prefixed(&mut encoded, suite)?;
        write_len_prefixed(&mut encoded, content_type)?;
        encoded.extend_from_slice(&self.plaintext_bytes.to_be_bytes());
        encoded.extend_from_slice(&self.base_nonce);
        encoded.extend_from_slice(self.content_key_commitment.as_bytes());
        Ok(encoded)
    }

    fn framed_prefix_bytes(&self) -> Result<Vec<u8>, CustodyError> {
        let encoded = self.encoded_bytes()?;
        let mut prefix =
            Vec::with_capacity(PAYLOAD_MAGIC_V1.len() + PAYLOAD_HEADER_LENGTH_BYTES_V1);
        prefix.extend_from_slice(&PAYLOAD_MAGIC_V1);
        let header_len = u16::try_from(encoded.len())
            .map_err(|_| CustodyError::InvalidPayload("header_bytes"))?;
        prefix.extend_from_slice(&header_len.to_be_bytes());
        prefix.extend_from_slice(&encoded);
        Ok(prefix)
    }

    fn expected_framed_bytes(&self) -> Result<u64, CustodyError> {
        self.validate_basic_fields()?;
        let header_bytes = u64::try_from(self.encoded_len())
            .map_err(|_| CustodyError::InvalidPayload("header_bytes"))?;
        let prefix_bytes = u64::try_from(PAYLOAD_MAGIC_V1.len() + PAYLOAD_HEADER_LENGTH_BYTES_V1)
            .map_err(|_| CustodyError::InvalidPayload("header_bytes"))?;
        let tag_bytes = self
            .chunk_count()?
            .checked_mul(u64::try_from(PAYLOAD_TAG_BYTES_V1).unwrap())
            .ok_or(CustodyError::InvalidPayload("framed_ciphertext_bytes"))?;
        prefix_bytes
            .checked_add(header_bytes)
            .and_then(|value| value.checked_add(self.plaintext_bytes))
            .and_then(|value| value.checked_add(tag_bytes))
            .ok_or(CustodyError::InvalidPayload("framed_ciphertext_bytes"))
    }

    #[cfg(test)]
    fn parse_framed(bytes: &[u8]) -> Result<(Self, usize), CustodyError> {
        if bytes.len() < PAYLOAD_MAGIC_V1.len() + PAYLOAD_HEADER_LENGTH_BYTES_V1 {
            return Err(CustodyError::InvalidPayload("framed_ciphertext_bytes"));
        }
        if bytes[..PAYLOAD_MAGIC_V1.len()] != PAYLOAD_MAGIC_V1 {
            return Err(CustodyError::InvalidPayload("payload_magic"));
        }
        let header_len_offset = PAYLOAD_MAGIC_V1.len();
        let header_len = u16::from_be_bytes(
            bytes[header_len_offset..header_len_offset + PAYLOAD_HEADER_LENGTH_BYTES_V1]
                .try_into()
                .unwrap(),
        ) as usize;
        let prefix_end = PAYLOAD_MAGIC_V1
            .len()
            .checked_add(PAYLOAD_HEADER_LENGTH_BYTES_V1)
            .and_then(|value| value.checked_add(header_len))
            .ok_or(CustodyError::InvalidPayload("header_bytes"))?;
        if prefix_end > bytes.len() {
            return Err(CustodyError::InvalidPayload("header_bytes"));
        }
        let header = Self::decode_bytes(
            &bytes[PAYLOAD_MAGIC_V1.len() + PAYLOAD_HEADER_LENGTH_BYTES_V1..prefix_end],
        )?;
        let expected_len = usize::try_from(header.expected_framed_bytes()?)
            .map_err(|_| CustodyError::InvalidPayload("framed_ciphertext_bytes"))?;
        if bytes.len() != expected_len {
            return Err(CustodyError::InvalidPayload("framed_ciphertext_bytes"));
        }
        Ok((header, prefix_end))
    }

    #[cfg(test)]
    fn decode_bytes(bytes: &[u8]) -> Result<Self, CustodyError> {
        let mut offset = 0usize;
        let schema = read_len_prefixed(bytes, &mut offset, "schema")?;
        let suite = read_len_prefixed(bytes, &mut offset, "suite_id")?;
        let content_type = read_len_prefixed(bytes, &mut offset, "content_type")?;
        let plaintext_bytes = read_u64(bytes, &mut offset, "plaintext_bytes")?;
        let base_nonce =
            read_fixed::<PAYLOAD_BASE_NONCE_BYTES_V1>(bytes, &mut offset, "base_nonce")?;
        let content_key_commitment = Digest32::new(read_fixed::<32>(
            bytes,
            &mut offset,
            "content_key_commitment",
        )?);
        if offset != bytes.len() {
            return Err(CustodyError::InvalidPayload("header_bytes"));
        }
        if schema != PAYLOAD_SCHEMA_V1.as_bytes() {
            return Err(CustodyError::InvalidPayload("schema"));
        }
        if suite != PAYLOAD_SUITE_ID_V1.as_bytes() {
            return Err(CustodyError::InvalidPayload("suite_id"));
        }
        Self::new_authenticated(
            String::from_utf8(content_type.to_vec())
                .map_err(|_| CustodyError::InvalidPayload("content_type"))?,
            plaintext_bytes,
            base_nonce,
            content_key_commitment,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedPayloadMetadataV1 {
    header: AuthenticatedChunkPayloadHeaderV1,
    encrypted_content_identity: EncryptedContentIdentityV1,
    custody_envelope: CustodyEnvelopeV1,
}

struct PayloadSealContextV1 {
    content_key: ContentEncryptionKeyV1,
    base_nonce: [u8; PAYLOAD_BASE_NONCE_BYTES_V1],
    custody_epoch: CustodyEpochIdentityV1,
    threshold: ThresholdV1,
    node_keys: Vec<(NodePublicKey, NodeCustodyPublicKeyV1)>,
}

impl SealedPayloadMetadataV1 {
    pub const fn header(&self) -> &AuthenticatedChunkPayloadHeaderV1 {
        &self.header
    }

    pub const fn encrypted_content_identity(&self) -> &EncryptedContentIdentityV1 {
        &self.encrypted_content_identity
    }

    pub const fn custody_envelope(&self) -> &CustodyEnvelopeV1 {
        &self.custody_envelope
    }
}

/// Seal one canonical framed ciphertext object into a caller-owned staging
/// writer and provision the matching custody envelope. The content key and
/// nonce are generated internally and never returned.
///
/// On any error this function returns no publishable metadata. Callers must
/// discard the staging output instead of treating it as a complete object.
pub fn seal_payload_to_staging_writer_v1<R: Read, W: Write>(
    content_type: &str,
    plaintext_bytes: u64,
    plaintext: &mut R,
    staging_ciphertext: &mut W,
    custody_epoch: CustodyEpochIdentityV1,
    threshold: ThresholdV1,
    node_keys: Vec<(NodePublicKey, NodeCustodyPublicKeyV1)>,
) -> Result<SealedPayloadMetadataV1, CustodyError> {
    let context = PayloadSealContextV1 {
        content_key: ContentEncryptionKeyV1::generate()?,
        base_nonce: random_base_nonce()?,
        custody_epoch,
        threshold,
        node_keys,
    };
    seal_payload_to_staging_writer_inner(
        content_type,
        plaintext_bytes,
        plaintext,
        staging_ciphertext,
        &context,
    )
}

fn seal_payload_to_staging_writer_inner<R: Read, W: Write>(
    content_type: &str,
    plaintext_bytes: u64,
    plaintext: &mut R,
    staging_ciphertext: &mut W,
    context: &PayloadSealContextV1,
) -> Result<SealedPayloadMetadataV1, CustodyError> {
    let header = AuthenticatedChunkPayloadHeaderV1::new_authenticated(
        content_type,
        plaintext_bytes,
        context.base_nonce,
        context.content_key.commitment(),
    )?;
    let header_bytes = header.encoded_bytes()?;
    let prefix_bytes = header.framed_prefix_bytes()?;
    let chunk_count = header.chunk_count()?;

    let mut hasher = Sha256::new();
    let mut written_bytes = 0u64;

    write_all_counted(
        staging_ciphertext,
        &prefix_bytes,
        &mut hasher,
        &mut written_bytes,
    )?;

    {
        // Keep the expanded AEAD state inside the shortest possible scope.
        // This crate explicitly zeroizes CEK bytes and plaintext chunk buffers.
        // The enabled upstream `aes` zeroize support clears AES round keys
        // where that dependency implements it. However, the composite
        // `Aes256Gcm`/GHASH state does not expose a complete public
        // zeroization contract across all backends; notably, the AArch64 PMULL
        // POLYVAL path does not provide a full zeroizing `Drop`. We therefore
        // keep the cipher lifetime as short as possible and report any stronger
        // whole-AEAD-state erasure claim as unsupported by the current audited
        // primitive stack.
        let cipher = context.content_key.with_bytes(|bytes| {
            Aes256Gcm::new_from_slice(bytes)
                .map_err(|_| CustodyError::InvalidPayload("content_key"))
        })?;

        for chunk_index in 0..chunk_count {
            let chunk_len = chunk_plaintext_len(&header, chunk_index)?;
            let mut plaintext_chunk = Zeroizing::new(vec![0u8; chunk_len]);
            read_exact_plaintext(plaintext, plaintext_chunk.as_mut_slice())?;
            let nonce = derive_chunk_nonce(&header, chunk_index)?;
            let aad = chunk_aad_bytes(&header_bytes, chunk_index);
            let encrypted_chunk = cipher
                .encrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: plaintext_chunk.as_slice(),
                        aad: &aad,
                    },
                )
                .map_err(|_| CustodyError::InvalidPayload("payload_chunk"))?;
            write_all_counted(
                staging_ciphertext,
                &encrypted_chunk,
                &mut hasher,
                &mut written_bytes,
            )?;
        }
    }

    reject_trailing_plaintext(plaintext)?;

    let expected_bytes = header.expected_framed_bytes()?;
    if written_bytes != expected_bytes {
        return Err(CustodyError::InvalidPayload("framed_ciphertext_bytes"));
    }

    let encrypted_content_identity =
        EncryptedContentIdentityV1::new(Digest32::new(hasher.finalize().into()), written_bytes)?;
    let custody_envelope = crate::provision_custody_envelope(
        encrypted_content_identity.clone(),
        &context.content_key,
        context.custody_epoch,
        context.threshold,
        context.node_keys.clone(),
    )?;
    Ok(SealedPayloadMetadataV1 {
        header,
        encrypted_content_identity,
        custody_envelope,
    })
}

fn random_base_nonce() -> Result<[u8; PAYLOAD_BASE_NONCE_BYTES_V1], CustodyError> {
    let mut nonce = [0u8; PAYLOAD_BASE_NONCE_BYTES_V1];
    StdRng::try_from_os_rng()
        .map_err(|_| CustodyError::RandomnessUnavailable)?
        .fill_bytes(&mut nonce);
    Ok(nonce)
}

fn derive_chunk_nonce(
    header: &AuthenticatedChunkPayloadHeaderV1,
    chunk_index: u64,
) -> Result<[u8; PAYLOAD_BASE_NONCE_BYTES_V1], CustodyError> {
    if chunk_index >= header.chunk_count()? {
        return Err(CustodyError::InvalidPayload("chunk_index"));
    }
    let mut nonce = *header.base_nonce();
    for (slot, byte) in nonce[4..].iter_mut().zip(chunk_index.to_be_bytes()) {
        *slot ^= byte;
    }
    Ok(nonce)
}

fn chunk_aad_bytes(header_bytes: &[u8], chunk_index: u64) -> Vec<u8> {
    let mut aad =
        Vec::with_capacity(PAYLOAD_CHUNK_AAD_DOMAIN_V1.len() + 1 + header_bytes.len() + 8);
    aad.extend_from_slice(PAYLOAD_CHUNK_AAD_DOMAIN_V1);
    aad.push(0);
    aad.extend_from_slice(header_bytes);
    aad.extend_from_slice(&chunk_index.to_be_bytes());
    aad
}

fn chunk_plaintext_len(
    header: &AuthenticatedChunkPayloadHeaderV1,
    chunk_index: u64,
) -> Result<usize, CustodyError> {
    if chunk_index >= header.chunk_count()? {
        return Err(CustodyError::InvalidPayload("chunk_index"));
    }
    let chunk_bytes = u64::from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1);
    let start = chunk_index
        .checked_mul(chunk_bytes)
        .ok_or(CustodyError::InvalidPayload("chunk_index"))?;
    let remaining = header
        .plaintext_bytes()
        .checked_sub(start)
        .ok_or(CustodyError::InvalidPayload("chunk_index"))?;
    usize::try_from(remaining.min(chunk_bytes))
        .map_err(|_| CustodyError::InvalidPayload("chunk_index"))
}

#[cfg(test)]
fn chunk_ciphertext_range(
    header: &AuthenticatedChunkPayloadHeaderV1,
    prefix_end: usize,
    chunk_index: u64,
) -> Result<std::ops::Range<usize>, CustodyError> {
    if chunk_index >= header.chunk_count()? {
        return Err(CustodyError::InvalidPayload("chunk_index"));
    }
    let mut start = prefix_end;
    for prior in 0..chunk_index {
        start = start
            .checked_add(chunk_plaintext_len(header, prior)?)
            .and_then(|value| value.checked_add(PAYLOAD_TAG_BYTES_V1))
            .ok_or(CustodyError::InvalidPayload("framed_ciphertext_bytes"))?;
    }
    let end = start
        .checked_add(chunk_plaintext_len(header, chunk_index)?)
        .and_then(|value| value.checked_add(PAYLOAD_TAG_BYTES_V1))
        .ok_or(CustodyError::InvalidPayload("framed_ciphertext_bytes"))?;
    Ok(start..end)
}

fn write_len_prefixed(output: &mut Vec<u8>, bytes: &[u8]) -> Result<(), CustodyError> {
    let len =
        u16::try_from(bytes.len()).map_err(|_| CustodyError::InvalidPayload("header_bytes"))?;
    output.extend_from_slice(&len.to_be_bytes());
    output.extend_from_slice(bytes);
    Ok(())
}

#[cfg(test)]
fn read_len_prefixed<'a>(
    bytes: &'a [u8],
    offset: &mut usize,
    field: &'static str,
) -> Result<&'a [u8], CustodyError> {
    let len = usize::from(read_u16(bytes, offset, field)?);
    let end = offset
        .checked_add(len)
        .ok_or(CustodyError::InvalidPayload(field))?;
    if end > bytes.len() {
        return Err(CustodyError::InvalidPayload(field));
    }
    let slice = &bytes[*offset..end];
    *offset = end;
    Ok(slice)
}

#[cfg(test)]
fn read_u16(bytes: &[u8], offset: &mut usize, field: &'static str) -> Result<u16, CustodyError> {
    Ok(u16::from_be_bytes(read_fixed::<2>(bytes, offset, field)?))
}

#[cfg(test)]
fn read_u64(bytes: &[u8], offset: &mut usize, field: &'static str) -> Result<u64, CustodyError> {
    Ok(u64::from_be_bytes(read_fixed::<8>(bytes, offset, field)?))
}

#[cfg(test)]
fn read_fixed<const N: usize>(
    bytes: &[u8],
    offset: &mut usize,
    field: &'static str,
) -> Result<[u8; N], CustodyError> {
    let end = offset
        .checked_add(N)
        .ok_or(CustodyError::InvalidPayload(field))?;
    if end > bytes.len() {
        return Err(CustodyError::InvalidPayload(field));
    }
    let value = bytes[*offset..end]
        .try_into()
        .map_err(|_| CustodyError::InvalidPayload(field))?;
    *offset = end;
    Ok(value)
}

fn write_all_counted(
    writer: &mut impl Write,
    bytes: &[u8],
    hasher: &mut Sha256,
    written_bytes: &mut u64,
) -> Result<(), CustodyError> {
    writer
        .write_all(bytes)
        .map_err(|_| CustodyError::PayloadIo)?;
    hasher.update(bytes);
    *written_bytes = written_bytes
        .checked_add(u64::try_from(bytes.len()).unwrap())
        .ok_or(CustodyError::InvalidPayload("framed_ciphertext_bytes"))?;
    Ok(())
}

fn read_exact_plaintext(reader: &mut impl Read, buffer: &mut [u8]) -> Result<(), CustodyError> {
    let mut offset = 0usize;
    while offset < buffer.len() {
        match reader.read(&mut buffer[offset..]) {
            Ok(0) => return Err(CustodyError::InvalidPayload("plaintext_bytes")),
            Ok(read) => {
                offset = offset
                    .checked_add(read)
                    .ok_or(CustodyError::InvalidPayload("plaintext_bytes"))?;
            }
            Err(_) => return Err(CustodyError::PayloadIo),
        }
    }
    Ok(())
}

fn reject_trailing_plaintext(reader: &mut impl Read) -> Result<(), CustodyError> {
    let mut trailing = [0u8; 1];
    match reader.read(&mut trailing) {
        Ok(0) => Ok(()),
        Ok(_) => Err(CustodyError::InvalidPayload("plaintext_bytes")),
        Err(_) => Err(CustodyError::PayloadIo),
    }
}

#[cfg(test)]
fn open_payload_for_tests(
    framed: &[u8],
    content_key: &ContentEncryptionKeyV1,
) -> Result<(AuthenticatedChunkPayloadHeaderV1, Vec<u8>), CustodyError> {
    let (header, prefix_end) = AuthenticatedChunkPayloadHeaderV1::parse_framed(framed)?;
    if !content_key.matches_commitment(header.content_key_commitment) {
        return Err(CustodyError::ContentKeyCommitmentMismatch);
    }
    let cipher = content_key.with_bytes(|bytes| {
        Aes256Gcm::new_from_slice(bytes).map_err(|_| CustodyError::InvalidPayload("content_key"))
    })?;
    let header_bytes = header.encoded_bytes()?;
    let plaintext_capacity = usize::try_from(header.plaintext_bytes())
        .map_err(|_| CustodyError::InvalidPayload("plaintext_bytes"))?;
    let mut plaintext = Vec::with_capacity(plaintext_capacity);
    for chunk_index in 0..header.chunk_count()? {
        let range = chunk_ciphertext_range(&header, prefix_end, chunk_index)?;
        let nonce = derive_chunk_nonce(&header, chunk_index)?;
        let aad = chunk_aad_bytes(&header_bytes, chunk_index);
        let decrypted = Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: &framed[range],
                        aad: &aad,
                    },
                )
                .map_err(|_| CustodyError::InvalidPayload("payload_chunk"))?,
        );
        plaintext.extend_from_slice(decrypted.as_slice());
    }
    if u64::try_from(plaintext.len())
        .map_err(|_| CustodyError::InvalidPayload("plaintext_bytes"))?
        != header.plaintext_bytes()
    {
        return Err(CustodyError::InvalidPayload("plaintext_bytes"));
    }
    Ok((header, plaintext))
}

#[cfg(test)]
fn seal_payload_to_vec_with_test_material(
    content_type: &str,
    plaintext: &[u8],
    content_key: &ContentEncryptionKeyV1,
    base_nonce: [u8; PAYLOAD_BASE_NONCE_BYTES_V1],
) -> Result<(Vec<u8>, SealedPayloadMetadataV1), CustodyError> {
    let mut reader = std::io::Cursor::new(plaintext.to_vec());
    let mut framed = Vec::new();
    let metadata = seal_payload_to_staging_writer_inner(
        content_type,
        u64::try_from(plaintext.len()).unwrap(),
        &mut reader,
        &mut framed,
        &PayloadSealContextV1 {
            content_key: ContentEncryptionKeyV1::from_test_bytes(
                content_key.with_bytes(|bytes| *bytes),
            ),
            base_nonce,
            custody_epoch: crate::test_support::custody_epoch_identity(),
            threshold: ThresholdV1::new(2, 3)?,
            node_keys: crate::test_support::custody_nodes(),
        },
    )?;
    Ok((framed, metadata))
}

#[cfg(test)]
mod tests {
    use super::*;

    use elastos_protected_content_contracts::CanonicalContract;
    use std::collections::BTreeSet;

    fn node_keys() -> Vec<(NodePublicKey, NodeCustodyPublicKeyV1)> {
        crate::test_support::custody_nodes()
    }

    struct FailAfterWriter {
        limit: usize,
        written: usize,
    }

    impl Write for FailAfterWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.written >= self.limit {
                return Err(std::io::Error::other("writer blocked"));
            }
            let remaining = self.limit - self.written;
            let accepted = remaining.min(buf.len());
            self.written += accepted;
            Ok(accepted)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn payload_round_trips_through_private_verifier() {
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x44; 32]);
        let (framed, metadata) = seal_payload_to_vec_with_test_material(
            "application/octet-stream",
            b"hello protected content",
            &content_key,
            [0x51; PAYLOAD_BASE_NONCE_BYTES_V1],
        )
        .unwrap();

        let (header, plaintext) = open_payload_for_tests(&framed, &content_key).unwrap();
        assert_eq!(plaintext, b"hello protected content");
        assert_eq!(header, metadata.header().clone());
        assert_eq!(
            metadata.custody_envelope().manifest().encrypted_content(),
            metadata.encrypted_content_identity()
        );
        assert_eq!(
            metadata
                .custody_envelope()
                .manifest()
                .content_key_commitment(),
            metadata.header().content_key_commitment
        );
    }

    #[test]
    fn same_plaintext_seals_to_different_ciphertext_and_identity() {
        let plaintext = vec![0x33; 4096];
        let mut first_reader = std::io::Cursor::new(plaintext.clone());
        let mut second_reader = std::io::Cursor::new(plaintext);
        let mut first_framed = Vec::new();
        let mut second_framed = Vec::new();

        let first = seal_payload_to_staging_writer_v1(
            "application/octet-stream",
            4096,
            &mut first_reader,
            &mut first_framed,
            crate::test_support::custody_epoch_identity(),
            ThresholdV1::new(2, 3).unwrap(),
            node_keys(),
        )
        .unwrap();
        let second = seal_payload_to_staging_writer_v1(
            "application/octet-stream",
            4096,
            &mut second_reader,
            &mut second_framed,
            crate::test_support::custody_epoch_identity(),
            ThresholdV1::new(2, 3).unwrap(),
            node_keys(),
        )
        .unwrap();

        assert_ne!(first_framed, second_framed);
        assert_ne!(
            first.encrypted_content_identity(),
            second.encrypted_content_identity()
        );
    }

    #[test]
    fn payload_rejects_header_chunk_tag_order_duplication_splice_length_and_type_tampering() {
        let plaintext = vec![0x5a; usize::try_from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1).unwrap() * 2];
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x24; 32]);
        let (framed, metadata) = seal_payload_to_vec_with_test_material(
            "application/octet-stream",
            &plaintext,
            &content_key,
            [0x71; PAYLOAD_BASE_NONCE_BYTES_V1],
        )
        .unwrap();
        let (_, prefix_end) = AuthenticatedChunkPayloadHeaderV1::parse_framed(&framed).unwrap();
        let first_range = chunk_ciphertext_range(metadata.header(), prefix_end, 0).unwrap();
        let second_range = chunk_ciphertext_range(metadata.header(), prefix_end, 1).unwrap();

        let mut header_tampered = framed.clone();
        header_tampered[prefix_end - 1] ^= 0x01;
        assert!(open_payload_for_tests(&header_tampered, &content_key).is_err());

        let mut chunk_tampered = framed.clone();
        chunk_tampered[first_range.start] ^= 0x01;
        assert!(open_payload_for_tests(&chunk_tampered, &content_key).is_err());

        let mut tag_tampered = framed.clone();
        tag_tampered[first_range.end - 1] ^= 0x01;
        assert!(open_payload_for_tests(&tag_tampered, &content_key).is_err());

        let mut reordered = framed.clone();
        let first = framed[first_range.clone()].to_vec();
        let second = framed[second_range.clone()].to_vec();
        reordered[first_range.clone()].copy_from_slice(&second);
        reordered[second_range.clone()].copy_from_slice(&first);
        assert!(open_payload_for_tests(&reordered, &content_key).is_err());

        let mut duplicated = framed.clone();
        let first = framed[first_range.clone()].to_vec();
        duplicated[second_range.clone()].copy_from_slice(&first);
        assert!(open_payload_for_tests(&duplicated, &content_key).is_err());

        let other_plaintext =
            vec![0x6b; usize::try_from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1).unwrap() * 2];
        let (other_framed, _) = seal_payload_to_vec_with_test_material(
            "application/octet-stream",
            &other_plaintext,
            &content_key,
            [0x72; PAYLOAD_BASE_NONCE_BYTES_V1],
        )
        .unwrap();
        let mut spliced = framed.clone();
        spliced[second_range.clone()].copy_from_slice(&other_framed[second_range.clone()]);
        assert!(open_payload_for_tests(&spliced, &content_key).is_err());

        let mut wrong_length = framed.clone();
        let length_offset = PAYLOAD_MAGIC_V1.len()
            + PAYLOAD_HEADER_LENGTH_BYTES_V1
            + 2
            + PAYLOAD_SCHEMA_V1.len()
            + 2
            + PAYLOAD_SUITE_ID_V1.len()
            + 2
            + "application/octet-stream".len();
        wrong_length[length_offset + 7] ^= 0x01;
        assert!(AuthenticatedChunkPayloadHeaderV1::parse_framed(&wrong_length).is_err());

        let mut wrong_type = framed.clone();
        let type_offset = PAYLOAD_MAGIC_V1.len()
            + PAYLOAD_HEADER_LENGTH_BYTES_V1
            + 2
            + PAYLOAD_SCHEMA_V1.len()
            + 2
            + PAYLOAD_SUITE_ID_V1.len()
            + 2;
        wrong_type[type_offset] ^= 0x01;
        assert!(open_payload_for_tests(&wrong_type, &content_key).is_err());
    }

    #[test]
    fn payload_rejects_truncation_and_trailing_bytes() {
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x44; 32]);
        let (framed, _) = seal_payload_to_vec_with_test_material(
            "application/octet-stream",
            b"truncate me",
            &content_key,
            [0x61; PAYLOAD_BASE_NONCE_BYTES_V1],
        )
        .unwrap();
        assert!(
            AuthenticatedChunkPayloadHeaderV1::parse_framed(&framed[..framed.len() - 1]).is_err()
        );

        let mut trailing = framed.clone();
        trailing.push(0);
        assert!(AuthenticatedChunkPayloadHeaderV1::parse_framed(&trailing).is_err());
    }

    #[test]
    fn payload_rejects_wrong_key_and_wrong_commitment() {
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x44; 32]);
        let wrong_key = ContentEncryptionKeyV1::from_test_bytes([0x45; 32]);
        let (framed, metadata) = seal_payload_to_vec_with_test_material(
            "application/octet-stream",
            b"wrong key",
            &content_key,
            [0x62; PAYLOAD_BASE_NONCE_BYTES_V1],
        )
        .unwrap();

        assert!(matches!(
            open_payload_for_tests(&framed, &wrong_key),
            Err(CustodyError::ContentKeyCommitmentMismatch)
        ));

        let mut wrong_commitment = framed.clone();
        let commitment_offset = wrong_commitment.len()
            - usize::try_from(metadata.header().plaintext_bytes()).unwrap()
            - 32;
        wrong_commitment[commitment_offset] ^= 0x01;
        assert!(matches!(
            open_payload_for_tests(&wrong_commitment, &content_key),
            Err(CustodyError::ContentKeyCommitmentMismatch)
                | Err(CustodyError::InvalidPayload("payload_chunk"))
        ));
    }

    #[test]
    fn payload_rejects_zero_length_oversized_metadata_oversized_content_and_out_of_range_nonce_use_before_write(
    ) {
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x44; 32]);
        assert!(matches!(
            AuthenticatedChunkPayloadHeaderV1::new_authenticated(
                "application/octet-stream",
                0,
                [0x11; PAYLOAD_BASE_NONCE_BYTES_V1],
                content_key.commitment(),
            ),
            Err(CustodyError::InvalidPayload("plaintext_bytes"))
        ));
        assert!(matches!(
            AuthenticatedChunkPayloadHeaderV1::new_authenticated(
                "a".repeat(MAX_PAYLOAD_CONTENT_TYPE_BYTES_V1 + 1),
                1,
                [0x11; PAYLOAD_BASE_NONCE_BYTES_V1],
                content_key.commitment(),
            ),
            Err(CustodyError::InvalidPayload("content_type"))
        ));
        assert!(matches!(
            AuthenticatedChunkPayloadHeaderV1::new_authenticated(
                "application/octet-stream",
                MAX_ENCRYPTED_CONTENT_BYTES,
                [0x11; PAYLOAD_BASE_NONCE_BYTES_V1],
                content_key.commitment(),
            ),
            Err(CustodyError::InvalidPayload("framed_ciphertext_bytes"))
        ));

        let header = AuthenticatedChunkPayloadHeaderV1::new_authenticated(
            "application/octet-stream",
            1,
            [0x11; PAYLOAD_BASE_NONCE_BYTES_V1],
            content_key.commitment(),
        )
        .unwrap();
        assert!(matches!(
            derive_chunk_nonce(&header, 1),
            Err(CustodyError::InvalidPayload("chunk_index"))
        ));

        let mut reader = std::io::Cursor::new(vec![0u8; 1]);
        let mut writer = FailAfterWriter {
            limit: 0,
            written: 0,
        };
        let err = seal_payload_to_staging_writer_v1(
            "application/octet-stream",
            MAX_ENCRYPTED_CONTENT_BYTES,
            &mut reader,
            &mut writer,
            crate::test_support::custody_epoch_identity(),
            ThresholdV1::new(2, 3).unwrap(),
            node_keys(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CustodyError::InvalidPayload("framed_ciphertext_bytes")
        ));
        assert_eq!(writer.written, 0);
    }

    #[test]
    fn payload_nonces_are_unique_for_every_valid_chunk_index_under_the_maximum_framed_bound() {
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x44; 32]);
        let base_nonce = [0x13; PAYLOAD_BASE_NONCE_BYTES_V1];
        let max_plaintext_bytes = max_valid_plaintext_bytes_for_tests(
            "application/octet-stream",
            base_nonce,
            content_key.commitment(),
        );
        let header = AuthenticatedChunkPayloadHeaderV1::new_authenticated(
            "application/octet-stream",
            max_plaintext_bytes,
            base_nonce,
            content_key.commitment(),
        )
        .unwrap();
        let chunk_count = header.chunk_count().unwrap();
        let last_chunk_index = chunk_count - 1;
        // XOR with a fixed 64-bit suffix is a bijection over the u64 index
        // space, so recovering the original index from representative boundary
        // values proves nonce uniqueness across the full valid domain.
        let sample_indices = [
            0,
            1,
            chunk_count / 2,
            last_chunk_index.saturating_sub(1),
            last_chunk_index,
        ];
        let mut nonces = BTreeSet::new();
        for chunk_index in sample_indices {
            let nonce = derive_chunk_nonce(&header, chunk_index).unwrap();
            assert_eq!(
                recover_chunk_index_from_nonce_for_tests(base_nonce, nonce),
                chunk_index
            );
            assert!(nonces.insert(nonce));
        }
        assert_eq!(
            nonces.len(),
            sample_indices.into_iter().collect::<BTreeSet<_>>().len()
        );
        assert!(matches!(
            AuthenticatedChunkPayloadHeaderV1::new_authenticated(
                "application/octet-stream",
                max_plaintext_bytes + 1,
                [0x13; PAYLOAD_BASE_NONCE_BYTES_V1],
                content_key.commitment(),
            ),
            Err(CustodyError::InvalidPayload("framed_ciphertext_bytes"))
        ));
        assert!(matches!(
            derive_chunk_nonce(&header, chunk_count),
            Err(CustodyError::InvalidPayload("chunk_index"))
        ));
    }

    #[test]
    fn payload_debug_and_public_outputs_do_not_expose_content_key_bytes() {
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x44; 32]);
        let (framed, metadata) = seal_payload_to_vec_with_test_material(
            "application/octet-stream",
            b"debug me",
            &content_key,
            [0x63; PAYLOAD_BASE_NONCE_BYTES_V1],
        )
        .unwrap();
        let debug = format!("{metadata:?}");
        let key_bytes = [0x44; 32];
        assert!(!debug.contains(&"44".repeat(32)));
        assert!(!framed
            .windows(key_bytes.len())
            .any(|window| window == key_bytes));
        assert!(!metadata
            .custody_envelope()
            .canonical_bytes()
            .unwrap()
            .windows(key_bytes.len())
            .any(|window| window == key_bytes));
    }

    #[test]
    fn payload_partial_writer_failure_returns_no_complete_object() {
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x44; 32]);
        let mut reader = std::io::Cursor::new(vec![0x11; 4096]);
        let mut writer = FailAfterWriter {
            limit: PAYLOAD_MAGIC_V1.len() + PAYLOAD_HEADER_LENGTH_BYTES_V1 + 32,
            written: 0,
        };
        let err = seal_payload_to_staging_writer_inner(
            "application/octet-stream",
            4096,
            &mut reader,
            &mut writer,
            &PayloadSealContextV1 {
                content_key,
                base_nonce: [0x64; PAYLOAD_BASE_NONCE_BYTES_V1],
                custody_epoch: crate::test_support::custody_epoch_identity(),
                threshold: ThresholdV1::new(2, 3).unwrap(),
                node_keys: node_keys(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, CustodyError::PayloadIo));
    }

    #[test]
    fn payload_custody_provision_failure_returns_no_publishable_metadata() {
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x44; 32]);
        let plaintext = vec![0x31; 4096];
        let mut reader = std::io::Cursor::new(plaintext.clone());
        let mut staged = Vec::new();
        let mut invalid_node_keys = node_keys();
        invalid_node_keys[1].1 = invalid_node_keys[0].1;
        let err = seal_payload_to_staging_writer_inner(
            "application/octet-stream",
            4096,
            &mut reader,
            &mut staged,
            &PayloadSealContextV1 {
                content_key: ContentEncryptionKeyV1::from_test_bytes(
                    content_key.with_bytes(|bytes| *bytes),
                ),
                base_nonce: [0x65; PAYLOAD_BASE_NONCE_BYTES_V1],
                custody_epoch: crate::test_support::custody_epoch_identity(),
                threshold: ThresholdV1::new(2, 3).unwrap(),
                node_keys: invalid_node_keys,
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CustodyError::Contract(
                elastos_protected_content_contracts::ContractError::InvalidField(
                    "node_custody_public_key"
                )
            )
        ));
        let (header, decrypted) = open_payload_for_tests(&staged, &content_key).unwrap();
        assert_eq!(decrypted, plaintext);
        assert_eq!(
            u64::try_from(staged.len()).unwrap(),
            header.expected_framed_bytes().unwrap()
        );
    }

    fn max_valid_plaintext_bytes_for_tests(
        content_type: &str,
        base_nonce: [u8; PAYLOAD_BASE_NONCE_BYTES_V1],
        content_key_commitment: Digest32,
    ) -> u64 {
        let mut low = 1u64;
        let mut high = MAX_ENCRYPTED_CONTENT_BYTES;
        let mut best = 0u64;
        while low <= high {
            let midpoint = low + ((high - low) / 2);
            match AuthenticatedChunkPayloadHeaderV1::new_authenticated(
                content_type,
                midpoint,
                base_nonce,
                content_key_commitment,
            ) {
                Ok(_) => {
                    best = midpoint;
                    low = midpoint + 1;
                }
                Err(CustodyError::InvalidPayload("framed_ciphertext_bytes")) => {
                    high = midpoint - 1;
                }
                Err(err) => panic!("unexpected header validation error: {err:?}"),
            }
        }
        best
    }

    fn recover_chunk_index_from_nonce_for_tests(
        base_nonce: [u8; PAYLOAD_BASE_NONCE_BYTES_V1],
        nonce: [u8; PAYLOAD_BASE_NONCE_BYTES_V1],
    ) -> u64 {
        let mut index_bytes = [0u8; 8];
        for (slot, (base, derived)) in index_bytes
            .iter_mut()
            .zip(base_nonce[4..].iter().zip(nonce[4..].iter()))
        {
            *slot = *base ^ *derived;
        }
        u64::from_be_bytes(index_bytes)
    }
}
