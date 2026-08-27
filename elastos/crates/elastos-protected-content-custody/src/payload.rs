use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use rand09::{rngs::StdRng, RngCore as _, SeedableRng as _};
use sha2::{Digest as _, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use zeroize::Zeroizing;

use elastos_protected_content_contracts::{
    AuthenticatedRuntimeReleaseOperationV1, CustodyEnvelopeV1, Digest32,
    EncryptedContentIdentityV1, SignedNodeContributionV1, SignedTerminalReceiptV1,
    TerminalReceiptIssuerKey, ValidatedCustodyCommitteeV1, MAX_ENCRYPTED_CONTENT_BYTES,
};

use crate::{ContentEncryptionKeyV1, CustodyError, RecipientSecretKeyV1};

const PAYLOAD_MAGIC_V1: [u8; 4] = *b"EPC1";
const PAYLOAD_HEADER_LENGTH_BYTES_V1: usize = 2;
const PAYLOAD_BASE_NONCE_BYTES_V1: usize = 12;
const PAYLOAD_TAG_BYTES_V1: usize = 16;
const PAYLOAD_SCHEMA_V1: &str = "elastos.protected-content.payload/v1";
const PAYLOAD_SUITE_ID_V1: &str = "aes-256-gcm-chunked/v1";
const PAYLOAD_CHUNK_AAD_DOMAIN_V1: &[u8] = b"elastos.protected-content.payload.chunk-aad/v1";
const PAYLOAD_IDENTITY_VERIFY_BUFFER_BYTES_V1: usize = 64 * 1024;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedPayloadMetadataV1 {
    header: AuthenticatedChunkPayloadHeaderV1,
    encrypted_content_identity: EncryptedContentIdentityV1,
    custody_envelope: CustodyEnvelopeV1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecryptedPayloadMetadataV1 {
    content_type: String,
    plaintext_bytes: u64,
}

#[derive(Clone, Copy)]
pub struct AuthenticatedPayloadDecryptInputsV1<'a> {
    pub expected_encrypted_content_identity: &'a EncryptedContentIdentityV1,
    pub operation: &'a AuthenticatedRuntimeReleaseOperationV1,
    pub envelope: &'a CustodyEnvelopeV1,
    pub contributions: &'a [SignedNodeContributionV1],
    pub terminal_receipt: &'a SignedTerminalReceiptV1,
    pub expected_terminal_issuer: TerminalReceiptIssuerKey,
    pub recipient_secret: &'a RecipientSecretKeyV1,
    pub now: u64,
}

struct PayloadSealContextV1 {
    content_key: ContentEncryptionKeyV1,
    base_nonce: [u8; PAYLOAD_BASE_NONCE_BYTES_V1],
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

impl DecryptedPayloadMetadataV1 {
    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    pub const fn plaintext_bytes(&self) -> u64 {
        self.plaintext_bytes
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
    committee: &ValidatedCustodyCommitteeV1,
) -> Result<SealedPayloadMetadataV1, CustodyError> {
    let context = PayloadSealContextV1 {
        content_key: ContentEncryptionKeyV1::generate()?,
        base_nonce: random_base_nonce()?,
    };
    seal_payload_to_staging_writer_inner(
        content_type,
        plaintext_bytes,
        plaintext,
        staging_ciphertext,
        &context,
        committee,
    )
}

pub fn decrypt_payload_to_staging_writer_from_authenticated_operation_v1<
    R: Read + Seek,
    W: Write,
>(
    ciphertext_source: &mut R,
    plaintext_staging: &mut W,
    inputs: AuthenticatedPayloadDecryptInputsV1<'_>,
) -> Result<DecryptedPayloadMetadataV1, CustodyError> {
    if inputs.operation.binding().encrypted_content() != inputs.expected_encrypted_content_identity
    {
        return Err(CustodyError::BindingMismatch("encrypted_content"));
    }
    let content_key = crate::reconstruct_content_key_from_authenticated_operation(
        inputs.operation,
        inputs.envelope,
        inputs.contributions,
        inputs.terminal_receipt,
        inputs.expected_terminal_issuer,
        inputs.recipient_secret,
        inputs.now,
    )?;
    decrypt_payload_to_staging_writer_with_content_key_v1(
        inputs.expected_encrypted_content_identity,
        ciphertext_source,
        plaintext_staging,
        &content_key,
    )
}

fn seal_payload_to_staging_writer_inner<R: Read, W: Write>(
    content_type: &str,
    plaintext_bytes: u64,
    plaintext: &mut R,
    staging_ciphertext: &mut W,
    context: &PayloadSealContextV1,
    committee: &ValidatedCustodyCommitteeV1,
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
        committee,
    )?;
    Ok(SealedPayloadMetadataV1 {
        header,
        encrypted_content_identity,
        custody_envelope,
    })
}

fn decrypt_payload_to_staging_writer_with_content_key_v1<R: Read + Seek, W: Write>(
    expected_encrypted_content_identity: &EncryptedContentIdentityV1,
    ciphertext_source: &mut R,
    plaintext_staging: &mut W,
    content_key: &ContentEncryptionKeyV1,
) -> Result<DecryptedPayloadMetadataV1, CustodyError> {
    let (header, prefix_len) = verify_framed_ciphertext_identity_v1(
        ciphertext_source,
        expected_encrypted_content_identity,
    )?;
    if !content_key.matches_commitment(header.content_key_commitment) {
        return Err(CustodyError::ContentKeyCommitmentMismatch);
    }
    let header_bytes = header.encoded_bytes()?;
    let chunk_count = header.chunk_count()?;

    seek_to_start(ciphertext_source)?;
    ciphertext_source
        .seek(SeekFrom::Start(prefix_len))
        .map_err(|_| CustodyError::PayloadIo)?;

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
    let cipher = content_key.with_bytes(|bytes| {
        Aes256Gcm::new_from_slice(bytes).map_err(|_| CustodyError::InvalidPayload("content_key"))
    })?;

    for chunk_index in 0..chunk_count {
        let ciphertext_chunk_len = chunk_ciphertext_len(&header, chunk_index)?;
        let mut ciphertext_chunk = vec![0u8; ciphertext_chunk_len];
        read_exact_payload_bytes(
            ciphertext_source,
            &mut ciphertext_chunk,
            "framed_ciphertext_bytes",
        )?;
        let nonce = derive_chunk_nonce(&header, chunk_index)?;
        let aad = chunk_aad_bytes(&header_bytes, chunk_index);
        let plaintext_chunk = Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: &ciphertext_chunk,
                        aad: &aad,
                    },
                )
                .map_err(|_| CustodyError::InvalidPayload("payload_chunk"))?,
        );
        plaintext_staging
            .write_all(plaintext_chunk.as_slice())
            .map_err(|_| CustodyError::PayloadIo)?;
    }

    let mut trailing = [0u8; 1];
    match ciphertext_source.read(&mut trailing) {
        Ok(0) => Ok(DecryptedPayloadMetadataV1 {
            content_type: header.content_type().to_string(),
            plaintext_bytes: header.plaintext_bytes(),
        }),
        Ok(_) => Err(CustodyError::InvalidPayload("framed_ciphertext_bytes")),
        Err(_) => Err(CustodyError::PayloadIo),
    }
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

fn chunk_ciphertext_len(
    header: &AuthenticatedChunkPayloadHeaderV1,
    chunk_index: u64,
) -> Result<usize, CustodyError> {
    chunk_plaintext_len(header, chunk_index)?
        .checked_add(PAYLOAD_TAG_BYTES_V1)
        .ok_or(CustodyError::InvalidPayload("framed_ciphertext_bytes"))
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

fn read_u16(bytes: &[u8], offset: &mut usize, field: &'static str) -> Result<u16, CustodyError> {
    Ok(u16::from_be_bytes(read_fixed::<2>(bytes, offset, field)?))
}

fn read_u64(bytes: &[u8], offset: &mut usize, field: &'static str) -> Result<u64, CustodyError> {
    Ok(u64::from_be_bytes(read_fixed::<8>(bytes, offset, field)?))
}

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

fn seek_to_start(reader: &mut impl Seek) -> Result<(), CustodyError> {
    reader
        .seek(SeekFrom::Start(0))
        .map(|_| ())
        .map_err(|_| CustodyError::PayloadIo)
}

fn read_framed_header_from_reader_v1(
    reader: &mut (impl Read + Seek),
) -> Result<(AuthenticatedChunkPayloadHeaderV1, u64), CustodyError> {
    seek_to_start(reader)?;
    let mut magic = [0u8; PAYLOAD_MAGIC_V1.len()];
    read_exact_payload_bytes(reader, &mut magic, "framed_ciphertext_bytes")?;
    if magic != PAYLOAD_MAGIC_V1 {
        return Err(CustodyError::InvalidPayload("payload_magic"));
    }
    let mut header_len_bytes = [0u8; PAYLOAD_HEADER_LENGTH_BYTES_V1];
    read_exact_payload_bytes(reader, &mut header_len_bytes, "header_bytes")?;
    let header_len = usize::from(u16::from_be_bytes(header_len_bytes));
    let mut header_bytes = vec![0u8; header_len];
    read_exact_payload_bytes(reader, &mut header_bytes, "header_bytes")?;
    let header = decode_header_bytes_v1(&header_bytes)?;
    let prefix_len = u64::try_from(PAYLOAD_MAGIC_V1.len() + PAYLOAD_HEADER_LENGTH_BYTES_V1)
        .unwrap()
        .checked_add(u64::try_from(header_len).unwrap())
        .ok_or(CustodyError::InvalidPayload("header_bytes"))?;
    Ok((header, prefix_len))
}

fn verify_framed_ciphertext_identity_v1(
    reader: &mut (impl Read + Seek),
    expected_encrypted_content_identity: &EncryptedContentIdentityV1,
) -> Result<(AuthenticatedChunkPayloadHeaderV1, u64), CustodyError> {
    let (header, prefix_len) = read_framed_header_from_reader_v1(reader)?;
    let expected_framed_bytes = header.expected_framed_bytes()?;
    seek_to_start(reader)?;
    let mut hasher = Sha256::new();
    let mut total_bytes = 0u64;
    let mut buffer = [0u8; PAYLOAD_IDENTITY_VERIFY_BUFFER_BYTES_V1];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                hasher.update(&buffer[..read]);
                total_bytes = total_bytes
                    .checked_add(u64::try_from(read).unwrap())
                    .ok_or(CustodyError::InvalidPayload("framed_ciphertext_bytes"))?;
            }
            Err(_) => return Err(CustodyError::PayloadIo),
        }
    }
    if total_bytes != expected_framed_bytes {
        return Err(CustodyError::InvalidPayload("framed_ciphertext_bytes"));
    }
    let actual_encrypted_content_identity =
        EncryptedContentIdentityV1::new(Digest32::new(hasher.finalize().into()), total_bytes)?;
    if &actual_encrypted_content_identity != expected_encrypted_content_identity {
        return Err(CustodyError::BindingMismatch("encrypted_content"));
    }
    Ok((header, prefix_len))
}

fn read_exact_payload_bytes(
    reader: &mut impl Read,
    buffer: &mut [u8],
    field: &'static str,
) -> Result<(), CustodyError> {
    let mut offset = 0usize;
    while offset < buffer.len() {
        match reader.read(&mut buffer[offset..]) {
            Ok(0) => return Err(CustodyError::InvalidPayload(field)),
            Ok(read) => {
                offset = offset
                    .checked_add(read)
                    .ok_or(CustodyError::InvalidPayload(field))?;
            }
            Err(_) => return Err(CustodyError::PayloadIo),
        }
    }
    Ok(())
}

fn decode_header_bytes_v1(bytes: &[u8]) -> Result<AuthenticatedChunkPayloadHeaderV1, CustodyError> {
    let mut offset = 0usize;
    let schema = read_len_prefixed(bytes, &mut offset, "schema")?;
    let suite = read_len_prefixed(bytes, &mut offset, "suite_id")?;
    let content_type = read_len_prefixed(bytes, &mut offset, "content_type")?;
    let plaintext_bytes = read_u64(bytes, &mut offset, "plaintext_bytes")?;
    let base_nonce = read_fixed::<PAYLOAD_BASE_NONCE_BYTES_V1>(bytes, &mut offset, "base_nonce")?;
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
    AuthenticatedChunkPayloadHeaderV1::new_authenticated(
        String::from_utf8(content_type.to_vec())
            .map_err(|_| CustodyError::InvalidPayload("content_type"))?,
        plaintext_bytes,
        base_nonce,
        content_key_commitment,
    )
}

#[cfg(test)]
fn decrypt_payload_to_vec_with_content_key_for_tests(
    expected_encrypted_content_identity: &EncryptedContentIdentityV1,
    framed: &[u8],
    content_key: &ContentEncryptionKeyV1,
) -> Result<(DecryptedPayloadMetadataV1, Vec<u8>), CustodyError> {
    let mut reader = std::io::Cursor::new(framed.to_vec());
    let mut plaintext = Vec::new();
    let metadata = decrypt_payload_to_staging_writer_with_content_key_v1(
        expected_encrypted_content_identity,
        &mut reader,
        &mut plaintext,
        content_key,
    )?;
    Ok((metadata, plaintext))
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
        },
        &crate::test_support::validated_custody_committee(),
    )?;
    Ok((framed, metadata))
}

#[cfg(test)]
mod tests {
    use super::*;

    use ed25519_dalek::{Signer as _, SigningKey};
    use elastos_protected_content_contracts::{
        AtomicReplayClaimer, CanonicalContract, CustodyPoolError, CustodyPoolIdentityV1,
        CustodyPoolMemberStateV1, Digest32, EncryptedContentIdentityV1, EvmContractAddressV1,
        EvmFunctionSelectorV1, EvmRightsMethodAbiV1, KeyReleaseOutcomeV1, KeyReleaseRequestV1,
        NodeContributionRefV1, ProtectedContentBindingV1, RecipientKeyIdentityV1,
        RecipientPublicKeyBytesV1, ReplayClaimError, ReplayClaimKeyV1, ReplayNonce16,
        RightsActionV1, RightsObservationFinalityV1, RightsPolicyBodyV1, RightsRequestV1,
        RightsSubjectSourceV1, RightsVerificationContextV1, RuntimeOperationIssuerKeyV1,
        RuntimeReleaseAuditIdV1, RuntimeReleaseOperationStatementV1, RuntimeSessionBindingV1,
        SignedNodeContributionV1, SignedRecipientKeyAuthorizationV1,
        SignedRuntimeReleaseOperationV1, SignedTerminalReceiptV1, TerminalReceiptIssuerKey,
        TerminalReceiptStatementV1, VerifiedKeyReleaseRequestV1, WalletAddress,
        WalletSignedRightsRequestV1, CUSTODY_HPKE_SUITE_ID_V1,
    };
    use k256::ecdsa::SigningKey as WalletSigningKey;
    use rand09::rngs::StdRng;
    use sha3::Keccak256;
    use std::collections::{BTreeSet, HashMap};

    #[derive(Default)]
    struct ReplayClaimsForTests(HashMap<ReplayClaimKeyV1, u64>);

    impl AtomicReplayClaimer for ReplayClaimsForTests {
        fn claim(
            &mut self,
            key: ReplayClaimKeyV1,
            expires_at: u64,
            now: u64,
        ) -> Result<(), ReplayClaimError> {
            self.0.retain(|_, expiry| *expiry > now);
            if self.0.contains_key(&key) {
                return Err(ReplayClaimError::AlreadyClaimed);
            }
            self.0.insert(key, expires_at);
            Ok(())
        }
    }

    fn payload_identity_for_tests(framed: &[u8]) -> EncryptedContentIdentityV1 {
        EncryptedContentIdentityV1::new(
            Digest32::new(Sha256::digest(framed).into()),
            u64::try_from(framed.len()).unwrap(),
        )
        .unwrap()
    }

    fn framed_prefix_end_for_tests(framed: &[u8]) -> usize {
        let mut reader = std::io::Cursor::new(framed.to_vec());
        let (_, prefix_end) = read_framed_header_from_reader_v1(&mut reader).unwrap();
        usize::try_from(prefix_end).unwrap()
    }

    fn wallet_for_tests(seed: u8) -> WalletAddress {
        let key = WalletSigningKey::from_slice(&[seed; 32]).unwrap();
        let encoded = key.verifying_key().to_encoded_point(false);
        let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
        WalletAddress::new(digest[12..].try_into().unwrap())
    }

    fn policy_body_for_tests() -> RightsPolicyBodyV1 {
        RightsPolicyBodyV1::new(
            "content:alpha",
            RightsActionV1::View,
            "view",
            RightsSubjectSourceV1::WalletAddress,
            11155111,
            EvmContractAddressV1::new([0x11; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString,
            RightsObservationFinalityV1::new(12),
        )
        .unwrap()
    }

    fn recipient_identity_for_tests(seed: u8) -> RecipientKeyIdentityV1 {
        let recipient_public_key = crate::test_support::recipient_public_key(seed);
        RecipientKeyIdentityV1::new(
            CUSTODY_HPKE_SUITE_ID_V1,
            Digest32::new(sha2::Sha256::digest(recipient_public_key.as_bytes()).into()),
        )
        .unwrap()
    }

    fn binding_for_sealed_envelope_for_tests(
        encrypted_content_identity: EncryptedContentIdentityV1,
        envelope: &CustodyEnvelopeV1,
        wallet: WalletAddress,
    ) -> ProtectedContentBindingV1 {
        let policy_body = policy_body_for_tests();
        ProtectedContentBindingV1::new(
            encrypted_content_identity,
            envelope.key_envelope_identity().unwrap(),
            policy_body.policy_identity().unwrap(),
            elastos_protected_content_contracts::ProfileIdentityV1::from_public_key_bytes(
                SigningKey::from_bytes(&[0x26; 32])
                    .verifying_key()
                    .to_bytes(),
            )
            .unwrap(),
            wallet,
            RuntimeSessionBindingV1::new(crate::test_support::digest(0x66)).unwrap(),
        )
        .unwrap()
    }

    fn signed_rights_request_for_sealed_envelope_for_tests(
        encrypted_content_identity: EncryptedContentIdentityV1,
        envelope: &CustodyEnvelopeV1,
        recipient_seed: u8,
    ) -> WalletSignedRightsRequestV1 {
        let wallet = wallet_for_tests(7);
        let request = RightsRequestV1::new(
            binding_for_sealed_envelope_for_tests(encrypted_content_identity, envelope, wallet),
            RightsActionV1::View,
            recipient_identity_for_tests(recipient_seed),
            crate::test_support::NOW,
            crate::test_support::NOW + 180,
            ReplayNonce16::new([0x55; 16]),
        )
        .unwrap();
        let key = WalletSigningKey::from_slice(&[7; 32]).unwrap();
        let (signature, recovery_id) = key
            .sign_prehash_recoverable(&elastos_auth::ethereum_signed_message_hash(
                &request.canonical_bytes().unwrap(),
            ))
            .unwrap();
        let mut signature_bytes = signature.to_bytes().to_vec();
        signature_bytes.push(recovery_id.to_byte());
        WalletSignedRightsRequestV1::new(request, signature_bytes).unwrap()
    }

    fn verified_release_request_for_sealed_envelope_for_tests(
        encrypted_content_identity: EncryptedContentIdentityV1,
        envelope: &CustodyEnvelopeV1,
        recipient_seed: u8,
    ) -> VerifiedKeyReleaseRequestV1 {
        let signed = signed_rights_request_for_sealed_envelope_for_tests(
            encrypted_content_identity,
            envelope,
            recipient_seed,
        );
        let context = RightsVerificationContextV1::new(
            signed.request().binding().clone(),
            signed.request().action(),
            signed.request().recipient().clone(),
            crate::test_support::NOW + 1,
        );
        let rights = signed
            .verify(&context, &mut ReplayClaimsForTests::default())
            .unwrap();
        KeyReleaseRequestV1::new(
            rights.binding().clone(),
            rights.request_hash(),
            rights.action(),
            rights.recipient().clone(),
            crate::test_support::NOW + 1,
            crate::test_support::NOW + 50,
            ReplayNonce16::new([0x66; 16]),
        )
        .unwrap()
        .verify(
            &rights,
            crate::test_support::NOW + 3,
            &mut ReplayClaimsForTests::default(),
        )
        .unwrap()
    }

    fn authenticated_runtime_release_operation_for_sealed_envelope_for_tests(
        encrypted_content_identity: EncryptedContentIdentityV1,
        envelope: &CustodyEnvelopeV1,
        recipient_seed: u8,
    ) -> AuthenticatedRuntimeReleaseOperationV1 {
        let runtime_key = SigningKey::from_bytes(&[0x42; 32]);
        let recipient_public_key = crate::test_support::recipient_public_key(recipient_seed);
        let recipient_public_key_bytes =
            RecipientPublicKeyBytesV1::new(*recipient_public_key.as_bytes()).unwrap();
        let rights_request = signed_rights_request_for_sealed_envelope_for_tests(
            encrypted_content_identity,
            envelope,
            recipient_seed,
        );
        let release_request = KeyReleaseRequestV1::new(
            rights_request.request().binding().clone(),
            rights_request.request().request_hash().unwrap(),
            RightsActionV1::View,
            rights_request.request().recipient().clone(),
            crate::test_support::NOW + 1,
            crate::test_support::NOW + 50,
            ReplayNonce16::new([0x66; 16]),
        )
        .unwrap();
        let profile = SigningKey::from_bytes(&[0x26; 32]);
        let authorization_statement =
            elastos_protected_content_contracts::RecipientKeyAuthorizationStatementV1::new(
                rights_request.request().binding().clone(),
                RightsActionV1::View,
                recipient_public_key_bytes,
                rights_request.request().recipient().clone(),
                RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
                crate::test_support::NOW,
                crate::test_support::NOW + 90,
            )
            .unwrap();
        let authorization = SignedRecipientKeyAuthorizationV1::new(
            authorization_statement.clone(),
            profile
                .sign(&authorization_statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        let policy_body = policy_body_for_tests();
        let binding = rights_request.request().binding().clone();
        let statement = RuntimeReleaseOperationStatementV1::new(
            RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
            rights_request,
            release_request,
            recipient_public_key_bytes,
            authorization,
            policy_body.clone(),
            elastos_protected_content_contracts::RightsEvaluationEvidenceRequestV1::new(
                binding,
                policy_body.policy_identity().unwrap(),
            )
            .unwrap(),
            crate::test_support::signed_custody_epoch(),
            RuntimeReleaseAuditIdV1::new(crate::test_support::digest(0x91)).unwrap(),
            crate::test_support::NOW + 2,
            crate::test_support::NOW + 40,
        )
        .unwrap();
        SignedRuntimeReleaseOperationV1::new(
            statement.clone(),
            runtime_key
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
        .verify(
            statement.runtime_operation_issuer(),
            crate::test_support::NOW + 3,
        )
        .unwrap()
    }

    fn released_contribution(
        request: &VerifiedKeyReleaseRequestV1,
        envelope: &CustodyEnvelopeV1,
        encrypted_content_identity: EncryptedContentIdentityV1,
        node_seed: u8,
        recipient_seed: u8,
        hpke_seed: u8,
    ) -> SignedNodeContributionV1 {
        crate::release::produce_node_contribution_with_rng(
            &claimed_runtime_release_operation_for_sealed_envelope_and_node_seed(
                encrypted_content_identity,
                envelope,
                node_seed,
                recipient_seed,
            ),
            &crate::test_support::signed_node_decision(
                request,
                node_seed,
                elastos_protected_content_contracts::RightsDecisionV1::Allowed,
            ),
            &crate::NodeLocalStoredShareV1::extract_from_envelope(
                envelope,
                crate::test_support::node_public_key(node_seed),
            )
            .unwrap(),
            &crate::test_support::node_signing_key(node_seed),
            &crate::test_support::node_custody_secret(node_seed),
            &crate::test_support::recipient_public_key(recipient_seed),
            crate::test_support::NOW + 5,
            crate::test_support::NOW + 45,
            crate::test_support::NOW + 6,
            &mut StdRng::from_seed([hpke_seed; 32]),
        )
        .unwrap()
    }

    fn claimed_runtime_release_operation_for_sealed_envelope_and_node_seed(
        encrypted_content_identity: EncryptedContentIdentityV1,
        envelope: &CustodyEnvelopeV1,
        node_seed: u8,
        recipient_seed: u8,
    ) -> crate::replay_store::ClaimedNodeReleaseOperationV1 {
        let authenticated = authenticated_runtime_release_operation_for_sealed_envelope_for_tests(
            encrypted_content_identity,
            envelope,
            recipient_seed,
        );
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut store = crate::DurableReplayClaimStoreV1::new(
            crate::test_support::node_public_key(node_seed),
            temp.path().join("replay"),
        );
        store
            .claim_node_release_operation(
                authenticated,
                &crate::NodeLocalStoredShareV1::extract_from_envelope(
                    envelope,
                    crate::test_support::node_public_key(node_seed),
                )
                .unwrap(),
                crate::test_support::node_public_key(node_seed),
                crate::test_support::NOW + 3,
            )
            .unwrap()
    }

    fn terminal_receipt(
        request: &VerifiedKeyReleaseRequestV1,
        envelope: &CustodyEnvelopeV1,
        contributions: &[SignedNodeContributionV1],
        issuer_seed: u8,
        outcome: KeyReleaseOutcomeV1,
    ) -> SignedTerminalReceiptV1 {
        let verified = contributions
            .iter()
            .map(|contribution| {
                contribution
                    .verify(
                        request,
                        &envelope.manifest().node_set().unwrap(),
                        crate::test_support::NOW + 7,
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let issuer_key = SigningKey::from_bytes(&[issuer_seed; 32]);
        let issuer = TerminalReceiptIssuerKey::new(issuer_key.verifying_key().to_bytes()).unwrap();
        let refs = match outcome {
            KeyReleaseOutcomeV1::Denied => Vec::new(),
            KeyReleaseOutcomeV1::Released => {
                verified.iter().map(NodeContributionRefV1::from).collect()
            }
        };
        let statement = TerminalReceiptStatementV1::new(
            request.request_hash(),
            request.binding().clone(),
            issuer,
            outcome,
            refs,
            crate::test_support::NOW + 7,
            crate::test_support::NOW + 40,
        )
        .unwrap();
        SignedTerminalReceiptV1::new(
            statement.clone(),
            issuer_key
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    struct AuthenticatedDecryptFixture {
        framed: Vec<u8>,
        sealed: SealedPayloadMetadataV1,
        operation: AuthenticatedRuntimeReleaseOperationV1,
        contributions: Vec<SignedNodeContributionV1>,
        terminal: SignedTerminalReceiptV1,
        recipient_secret: RecipientSecretKeyV1,
        terminal_issuer: TerminalReceiptIssuerKey,
        plaintext: Vec<u8>,
    }

    fn authenticated_decrypt_fixture(plaintext: Vec<u8>) -> AuthenticatedDecryptFixture {
        let recipient_seed = 0x30;
        let mut plaintext_reader = std::io::Cursor::new(plaintext.clone());
        let mut framed = Vec::new();
        let sealed = seal_payload_to_staging_writer_v1(
            "application/octet-stream",
            u64::try_from(plaintext.len()).unwrap(),
            &mut plaintext_reader,
            &mut framed,
            &crate::test_support::validated_custody_committee(),
        )
        .unwrap();
        let operation = authenticated_runtime_release_operation_for_sealed_envelope_for_tests(
            sealed.encrypted_content_identity().clone(),
            sealed.custody_envelope(),
            recipient_seed,
        );
        let request = verified_release_request_for_sealed_envelope_for_tests(
            sealed.encrypted_content_identity().clone(),
            sealed.custody_envelope(),
            recipient_seed,
        );
        let contributions = vec![
            released_contribution(
                &request,
                sealed.custody_envelope(),
                sealed.encrypted_content_identity().clone(),
                1,
                recipient_seed,
                0x71,
            ),
            released_contribution(
                &request,
                sealed.custody_envelope(),
                sealed.encrypted_content_identity().clone(),
                2,
                recipient_seed,
                0x72,
            ),
        ];
        let terminal = terminal_receipt(
            &request,
            sealed.custody_envelope(),
            &contributions,
            0x21,
            KeyReleaseOutcomeV1::Released,
        );
        let terminal_issuer = terminal.statement().issuer();
        AuthenticatedDecryptFixture {
            framed,
            sealed,
            operation,
            contributions,
            terminal,
            recipient_secret: crate::test_support::recipient_secret(recipient_seed),
            terminal_issuer,
            plaintext,
        }
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
    fn payload_round_trips_through_production_decoder() {
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x44; 32]);
        let (framed, metadata) = seal_payload_to_vec_with_test_material(
            "application/octet-stream",
            b"hello protected content",
            &content_key,
            [0x51; PAYLOAD_BASE_NONCE_BYTES_V1],
        )
        .unwrap();

        let (decrypted, plaintext) = decrypt_payload_to_vec_with_content_key_for_tests(
            metadata.encrypted_content_identity(),
            &framed,
            &content_key,
        )
        .unwrap();
        assert_eq!(plaintext, b"hello protected content");
        assert_eq!(decrypted.content_type(), metadata.header().content_type());
        assert_eq!(
            decrypted.plaintext_bytes(),
            metadata.header().plaintext_bytes()
        );
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
            &crate::test_support::validated_custody_committee(),
        )
        .unwrap();
        let second = seal_payload_to_staging_writer_v1(
            "application/octet-stream",
            4096,
            &mut second_reader,
            &mut second_framed,
            &crate::test_support::validated_custody_committee(),
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
        let prefix_end = framed_prefix_end_for_tests(&framed);
        let first_range = chunk_ciphertext_range(metadata.header(), prefix_end, 0).unwrap();
        let second_range = chunk_ciphertext_range(metadata.header(), prefix_end, 1).unwrap();

        let mut header_tampered = framed.clone();
        header_tampered[prefix_end - 1] ^= 0x01;
        assert!(decrypt_payload_to_vec_with_content_key_for_tests(
            metadata.encrypted_content_identity(),
            &header_tampered,
            &content_key,
        )
        .is_err());

        let mut chunk_tampered = framed.clone();
        chunk_tampered[first_range.start] ^= 0x01;
        assert!(decrypt_payload_to_vec_with_content_key_for_tests(
            metadata.encrypted_content_identity(),
            &chunk_tampered,
            &content_key,
        )
        .is_err());

        let mut tag_tampered = framed.clone();
        tag_tampered[first_range.end - 1] ^= 0x01;
        assert!(decrypt_payload_to_vec_with_content_key_for_tests(
            metadata.encrypted_content_identity(),
            &tag_tampered,
            &content_key,
        )
        .is_err());

        let mut reordered = framed.clone();
        let first = framed[first_range.clone()].to_vec();
        let second = framed[second_range.clone()].to_vec();
        reordered[first_range.clone()].copy_from_slice(&second);
        reordered[second_range.clone()].copy_from_slice(&first);
        assert!(decrypt_payload_to_vec_with_content_key_for_tests(
            metadata.encrypted_content_identity(),
            &reordered,
            &content_key,
        )
        .is_err());

        let mut duplicated = framed.clone();
        let first = framed[first_range.clone()].to_vec();
        duplicated[second_range.clone()].copy_from_slice(&first);
        assert!(decrypt_payload_to_vec_with_content_key_for_tests(
            metadata.encrypted_content_identity(),
            &duplicated,
            &content_key,
        )
        .is_err());

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
        assert!(decrypt_payload_to_vec_with_content_key_for_tests(
            metadata.encrypted_content_identity(),
            &spliced,
            &content_key,
        )
        .is_err());

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
        assert!(decrypt_payload_to_vec_with_content_key_for_tests(
            metadata.encrypted_content_identity(),
            &wrong_length,
            &content_key,
        )
        .is_err());

        let mut wrong_type = framed.clone();
        let type_offset = PAYLOAD_MAGIC_V1.len()
            + PAYLOAD_HEADER_LENGTH_BYTES_V1
            + 2
            + PAYLOAD_SCHEMA_V1.len()
            + 2
            + PAYLOAD_SUITE_ID_V1.len()
            + 2;
        wrong_type[type_offset] ^= 0x01;
        assert!(decrypt_payload_to_vec_with_content_key_for_tests(
            metadata.encrypted_content_identity(),
            &wrong_type,
            &content_key,
        )
        .is_err());
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
        assert!(decrypt_payload_to_vec_with_content_key_for_tests(
            &payload_identity_for_tests(&framed),
            &framed[..framed.len() - 1],
            &content_key,
        )
        .is_err());

        let mut trailing = framed.clone();
        trailing.push(0);
        assert!(decrypt_payload_to_vec_with_content_key_for_tests(
            &payload_identity_for_tests(&framed),
            &trailing,
            &content_key,
        )
        .is_err());
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
            decrypt_payload_to_vec_with_content_key_for_tests(
                metadata.encrypted_content_identity(),
                &framed,
                &wrong_key,
            ),
            Err(CustodyError::ContentKeyCommitmentMismatch)
        ));

        let mut wrong_commitment = framed.clone();
        let commitment_offset = framed_prefix_end_for_tests(&wrong_commitment) - 32;
        wrong_commitment[commitment_offset] ^= 0x01;
        assert!(matches!(
            decrypt_payload_to_vec_with_content_key_for_tests(
                &payload_identity_for_tests(&wrong_commitment),
                &wrong_commitment,
                &content_key,
            ),
            Err(CustodyError::ContentKeyCommitmentMismatch)
        ));
    }

    #[test]
    fn decrypt_output_round_trips_through_sealing_reconstruction_and_authenticated_output() {
        let fixture =
            authenticated_decrypt_fixture(vec![
                0x7a;
                usize::try_from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1)
                    .unwrap()
                    + 257
            ]);
        let mut reader = std::io::Cursor::new(fixture.framed.clone());
        let mut plaintext = Vec::new();
        let metadata = decrypt_payload_to_staging_writer_from_authenticated_operation_v1(
            &mut reader,
            &mut plaintext,
            AuthenticatedPayloadDecryptInputsV1 {
                expected_encrypted_content_identity: fixture.sealed.encrypted_content_identity(),
                operation: &fixture.operation,
                envelope: fixture.sealed.custody_envelope(),
                contributions: &fixture.contributions,
                terminal_receipt: &fixture.terminal,
                expected_terminal_issuer: fixture.terminal_issuer,
                recipient_secret: &fixture.recipient_secret,
                now: crate::test_support::NOW + 8,
            },
        )
        .unwrap();
        assert_eq!(
            metadata.content_type(),
            fixture.sealed.header().content_type()
        );
        assert_eq!(
            metadata.plaintext_bytes(),
            fixture.sealed.header().plaintext_bytes()
        );
        assert_eq!(plaintext, fixture.plaintext);
    }

    #[test]
    fn decrypt_output_rejects_wrong_ciphertext_before_any_plaintext_write() {
        let fixture = authenticated_decrypt_fixture(b"identity mismatch".to_vec());
        let mut tampered = fixture.framed.clone();
        tampered[0] ^= 0x01;
        let mut reader = std::io::Cursor::new(tampered);
        let mut plaintext = Vec::new();
        let err = decrypt_payload_to_staging_writer_from_authenticated_operation_v1(
            &mut reader,
            &mut plaintext,
            AuthenticatedPayloadDecryptInputsV1 {
                expected_encrypted_content_identity: fixture.sealed.encrypted_content_identity(),
                operation: &fixture.operation,
                envelope: fixture.sealed.custody_envelope(),
                contributions: &fixture.contributions,
                terminal_receipt: &fixture.terminal,
                expected_terminal_issuer: fixture.terminal_issuer,
                recipient_secret: &fixture.recipient_secret,
                now: crate::test_support::NOW + 8,
            },
        )
        .unwrap_err();
        assert!(matches!(err, CustodyError::InvalidPayload("payload_magic")));
        assert!(plaintext.is_empty());
    }

    #[test]
    fn decrypt_output_rejects_wrong_operation_binding_and_invalid_contribution_sets() {
        let fixture = authenticated_decrypt_fixture(b"binding checks".to_vec());
        let mut wrong_operation_reader = std::io::Cursor::new(fixture.framed.clone());
        let wrong_operation = authenticated_runtime_release_operation_for_sealed_envelope_for_tests(
            fixture.sealed.encrypted_content_identity().clone(),
            fixture.sealed.custody_envelope(),
            0x31,
        );
        let mut plaintext = Vec::new();
        assert!(
            decrypt_payload_to_staging_writer_from_authenticated_operation_v1(
                &mut wrong_operation_reader,
                &mut plaintext,
                AuthenticatedPayloadDecryptInputsV1 {
                    expected_encrypted_content_identity: fixture
                        .sealed
                        .encrypted_content_identity(),
                    operation: &wrong_operation,
                    envelope: fixture.sealed.custody_envelope(),
                    contributions: &fixture.contributions,
                    terminal_receipt: &fixture.terminal,
                    expected_terminal_issuer: fixture.terminal_issuer,
                    recipient_secret: &fixture.recipient_secret,
                    now: crate::test_support::NOW + 8,
                },
            )
            .is_err()
        );
        assert!(plaintext.is_empty());

        let mut insufficient_reader = std::io::Cursor::new(fixture.framed.clone());
        let mut insufficient_plaintext = Vec::new();
        assert!(
            decrypt_payload_to_staging_writer_from_authenticated_operation_v1(
                &mut insufficient_reader,
                &mut insufficient_plaintext,
                AuthenticatedPayloadDecryptInputsV1 {
                    expected_encrypted_content_identity: fixture
                        .sealed
                        .encrypted_content_identity(),
                    operation: &fixture.operation,
                    envelope: fixture.sealed.custody_envelope(),
                    contributions: &fixture.contributions[..1],
                    terminal_receipt: &fixture.terminal,
                    expected_terminal_issuer: fixture.terminal_issuer,
                    recipient_secret: &fixture.recipient_secret,
                    now: crate::test_support::NOW + 8,
                },
            )
            .is_err()
        );
        assert!(insufficient_plaintext.is_empty());

        let mut duplicate_reader = std::io::Cursor::new(fixture.framed.clone());
        let mut duplicate_plaintext = Vec::new();
        let duplicate_contributions = vec![
            fixture.contributions[0].clone(),
            fixture.contributions[0].clone(),
        ];
        assert!(
            decrypt_payload_to_staging_writer_from_authenticated_operation_v1(
                &mut duplicate_reader,
                &mut duplicate_plaintext,
                AuthenticatedPayloadDecryptInputsV1 {
                    expected_encrypted_content_identity: fixture
                        .sealed
                        .encrypted_content_identity(),
                    operation: &fixture.operation,
                    envelope: fixture.sealed.custody_envelope(),
                    contributions: &duplicate_contributions,
                    terminal_receipt: &fixture.terminal,
                    expected_terminal_issuer: fixture.terminal_issuer,
                    recipient_secret: &fixture.recipient_secret,
                    now: crate::test_support::NOW + 8,
                },
            )
            .is_err()
        );
        assert!(duplicate_plaintext.is_empty());

        let mut mixed_reader = std::io::Cursor::new(fixture.framed);
        let mut mixed_plaintext = Vec::new();
        let wrong_request = verified_release_request_for_sealed_envelope_for_tests(
            fixture.sealed.encrypted_content_identity().clone(),
            fixture.sealed.custody_envelope(),
            0x31,
        );
        let mixed_contributions = vec![
            fixture.contributions[0].clone(),
            released_contribution(
                &wrong_request,
                fixture.sealed.custody_envelope(),
                fixture.sealed.encrypted_content_identity().clone(),
                2,
                0x31,
                0x73,
            ),
        ];
        assert!(
            decrypt_payload_to_staging_writer_from_authenticated_operation_v1(
                &mut mixed_reader,
                &mut mixed_plaintext,
                AuthenticatedPayloadDecryptInputsV1 {
                    expected_encrypted_content_identity: fixture
                        .sealed
                        .encrypted_content_identity(),
                    operation: &fixture.operation,
                    envelope: fixture.sealed.custody_envelope(),
                    contributions: &mixed_contributions,
                    terminal_receipt: &fixture.terminal,
                    expected_terminal_issuer: fixture.terminal_issuer,
                    recipient_secret: &fixture.recipient_secret,
                    now: crate::test_support::NOW + 8,
                },
            )
            .is_err()
        );
        assert!(mixed_plaintext.is_empty());
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
            &crate::test_support::validated_custody_committee(),
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
    fn decrypt_output_writer_failure_returns_no_success_metadata() {
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x44; 32]);
        let (framed, metadata) = seal_payload_to_vec_with_test_material(
            "application/octet-stream",
            &[0x33; 4096],
            &content_key,
            [0x73; PAYLOAD_BASE_NONCE_BYTES_V1],
        )
        .unwrap();
        let mut reader = std::io::Cursor::new(framed);
        let mut writer = FailAfterWriter {
            limit: 0,
            written: 0,
        };
        let err = decrypt_payload_to_staging_writer_with_content_key_v1(
            metadata.encrypted_content_identity(),
            &mut reader,
            &mut writer,
            &content_key,
        )
        .unwrap_err();
        assert!(matches!(err, CustodyError::PayloadIo));
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
            },
            &crate::test_support::validated_custody_committee(),
        )
        .unwrap_err();
        assert!(matches!(err, CustodyError::PayloadIo));
    }

    #[test]
    fn payload_rejects_invalid_committee_selection_before_staging_output() {
        let staged = Vec::<u8>::new();
        let epoch = crate::test_support::signed_custody_epoch();
        let original_pool = crate::test_support::signed_custody_pool();
        let original_authorization = crate::test_support::signed_committee_authorization(
            original_pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );
        let original_authorization_identity =
            original_authorization.authorization_identity().unwrap();

        let later_pool = crate::test_support::signed_custody_pool_with_member_state(
            CustodyPoolMemberStateV1::Active,
            (crate::test_support::NOW - 20, crate::test_support::NOW + 20),
        );
        let later_authorization = crate::test_support::signed_committee_authorization(
            later_pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );
        let wrong_pool_authorization = crate::test_support::signed_committee_authorization(
            CustodyPoolIdentityV1::new(Digest32::new([0xee; 32]), 123).unwrap(),
            epoch.epoch_identity().unwrap(),
        );

        let revoked_pool = crate::test_support::signed_custody_pool_with_member_state(
            CustodyPoolMemberStateV1::Revoked,
            (crate::test_support::NOW - 10, crate::test_support::NOW + 10),
        );
        let revoked_authorization = crate::test_support::signed_committee_authorization(
            revoked_pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );
        let expired_pool = crate::test_support::signed_custody_pool_with_member_state(
            CustodyPoolMemberStateV1::Active,
            (crate::test_support::NOW - 20, crate::test_support::NOW - 10),
        );
        let expired_authorization = crate::test_support::signed_committee_authorization(
            expired_pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );

        let cases = vec![
            (
                "revoked",
                revoked_pool,
                revoked_authorization.clone(),
                revoked_authorization.authorization_identity().unwrap(),
                Err(CustodyPoolError::Revoked),
            ),
            (
                "expired",
                expired_pool,
                expired_authorization.clone(),
                expired_authorization.authorization_identity().unwrap(),
                Err(CustodyPoolError::Expired),
            ),
            (
                "wrong_pool",
                later_pool,
                original_authorization,
                original_authorization_identity,
                Err(CustodyPoolError::BindingMismatch("custody_pool_identity")),
            ),
            (
                "wrong_authorization",
                original_pool,
                wrong_pool_authorization,
                later_authorization.authorization_identity().unwrap(),
                Err(CustodyPoolError::BindingMismatch(
                    "custody_committee_authorization_identity",
                )),
            ),
        ];

        for (label, pool, authorization, expected_identity, expected_error) in cases {
            let result =
                elastos_protected_content_contracts::validate_custody_epoch_against_pool_at(
                    crate::test_support::custody_policy_issuer(),
                    expected_identity,
                    &pool,
                    &epoch,
                    &authorization,
                    crate::test_support::NOW,
                );
            assert_eq!(result, expected_error, "{label}");
            assert!(
                staged.is_empty(),
                "{label} invalid selection must fail before staging output"
            );
        }
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
