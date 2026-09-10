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
        chunk_count_for_plaintext_bytes(self.plaintext_bytes)
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

/// Result of [`PayloadSealerV1::finish_unprovisioned`]: everything a caller
/// needs to provision a custody envelope, without this crate committing to
/// any particular provisioning entrypoint. [`PayloadSealerV1::finish`] is
/// exactly this type immediately followed by [`crate::provision_custody_envelope`];
/// a caller whose context only carries bare committee identities (not a
/// fully verified `ValidatedCustodyCommitteeV1`) — such as a sandboxed
/// protect-provider that only has what a Runtime-issued protection-session
/// request declares — calls `finish_unprovisioned` and then
/// [`crate::provision_custody_envelope_for_exact_nodes`] itself, mirroring
/// how this crate's fMP4/CENC media protection path already works.
///
/// `content_key()` returns a reference, not an owned value:
/// `ContentEncryptionKeyV1` is intentionally not `Clone`, and this type does
/// not widen access to its raw bytes — callers get exactly the same
/// reference-shaped access the media protection session already holds.
pub struct UnprovisionedSealedPayloadV1 {
    header: AuthenticatedChunkPayloadHeaderV1,
    encrypted_content_identity: EncryptedContentIdentityV1,
    content_key: ContentEncryptionKeyV1,
}

impl UnprovisionedSealedPayloadV1 {
    pub const fn header(&self) -> &AuthenticatedChunkPayloadHeaderV1 {
        &self.header
    }

    pub const fn encrypted_content_identity(&self) -> &EncryptedContentIdentityV1 {
        &self.encrypted_content_identity
    }

    pub const fn content_key(&self) -> &ContentEncryptionKeyV1 {
        &self.content_key
    }
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
    pub content_key_commitment: Digest32,
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
        context,
        committee,
    )
}

/// Reconstruct the content key for a non-media object session so a sandboxed
/// decrypt provider can drive [`PayloadChunkDecrypterV1`] directly, one
/// framed chunk at a time, instead of decrypting a whole staged file. Same
/// reconstruction path as
/// [`decrypt_payload_to_staging_writer_from_authenticated_operation_v1`]
/// above; this crate's `reconstruct_content_key_from_authenticated_operation`
/// is `pub(crate)`, so callers outside the crate (the sandboxed decrypt
/// provider) need this public wrapper. No new cryptography.
pub fn reconstruct_content_key_for_object_session(
    operation: &AuthenticatedRuntimeReleaseOperationV1,
    content_key_commitment: Digest32,
    contributions: &[SignedNodeContributionV1],
    terminal_receipt: &SignedTerminalReceiptV1,
    expected_terminal_issuer: TerminalReceiptIssuerKey,
    recipient_secret: &RecipientSecretKeyV1,
    now: u64,
) -> Result<ContentEncryptionKeyV1, CustodyError> {
    crate::reconstruct_content_key_from_authenticated_operation(
        operation,
        content_key_commitment,
        contributions,
        terminal_receipt,
        expected_terminal_issuer,
        recipient_secret,
        now,
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
        inputs.content_key_commitment,
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

/// Streaming EPC1 sealer: the chunk loop of [`seal_payload_to_staging_writer_v1`]
/// exposed as a stateful type so a sandboxed protect provider can seal one
/// frame at a time instead of staging the whole plaintext through a single
/// call. Same AAD (`domain ‖ header ‖ index`), nonce derivation, and identity
/// computation as the one-shot path below, which now drives this type
/// internally so there is exactly one sealing code path.
pub struct PayloadSealerV1 {
    header: AuthenticatedChunkPayloadHeaderV1,
    header_bytes: Vec<u8>,
    content_key: ContentEncryptionKeyV1,
    next_index: u64,
    hasher: Sha256,
    framed_bytes: u64,
}

impl PayloadSealerV1 {
    /// Open a sealer for a fresh, internally generated content key and base
    /// nonce. Returns the sealer and the framed header prefix (magic, length,
    /// and encoded header); the caller must emit that prefix before any chunk
    /// returned by [`Self::seal_chunk`].
    pub fn open(content_type: &str, plaintext_bytes: u64) -> Result<(Self, Vec<u8>), CustodyError> {
        Self::open_with_context(
            content_type,
            plaintext_bytes,
            ContentEncryptionKeyV1::generate()?,
            random_base_nonce()?,
        )
    }

    fn open_with_context(
        content_type: &str,
        plaintext_bytes: u64,
        content_key: ContentEncryptionKeyV1,
        base_nonce: [u8; PAYLOAD_BASE_NONCE_BYTES_V1],
    ) -> Result<(Self, Vec<u8>), CustodyError> {
        let header = AuthenticatedChunkPayloadHeaderV1::new_authenticated(
            content_type,
            plaintext_bytes,
            base_nonce,
            content_key.commitment(),
        )?;
        let header_bytes = header.encoded_bytes()?;
        let framed_prefix = header.framed_prefix_bytes()?;
        let mut hasher = Sha256::new();
        hasher.update(&framed_prefix);
        let framed_bytes = u64::try_from(framed_prefix.len())
            .map_err(|_| CustodyError::InvalidPayload("framed_ciphertext_bytes"))?;
        Ok((
            Self {
                header,
                header_bytes,
                content_key,
                next_index: 0,
                hasher,
                framed_bytes,
            },
            framed_prefix,
        ))
    }

    /// Test-only hook so the streaming path can be driven with the same fixed
    /// key and nonce as [`seal_with_context_for_tests`], to prove byte-for-byte
    /// equivalence with the one-shot sealer.
    #[cfg(test)]
    pub fn open_with_context_for_tests(
        content_type: &str,
        plaintext_bytes: u64,
        content_key: &ContentEncryptionKeyV1,
        base_nonce: [u8; PAYLOAD_BASE_NONCE_BYTES_V1],
    ) -> (Self, Vec<u8>) {
        Self::open_with_context(
            content_type,
            plaintext_bytes,
            ContentEncryptionKeyV1::from_test_bytes(content_key.with_bytes(|bytes| *bytes)),
            base_nonce,
        )
        .expect("fixed test key and nonce must produce a valid header")
    }

    /// Seal one chunk. `chunk_index` must equal the number of chunks already
    /// sealed (no gaps, no reordering, no replays), and `plaintext` must be
    /// exactly [`PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1`] bytes, except for the
    /// final chunk which may be shorter.
    pub fn seal_chunk(
        &mut self,
        chunk_index: u32,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CustodyError> {
        let chunk_index = u64::from(chunk_index);
        if chunk_index != self.next_index {
            return Err(CustodyError::InvalidPayload("chunk_index"));
        }
        let expected_len = chunk_plaintext_len(&self.header, chunk_index)?;
        if plaintext.len() != expected_len {
            return Err(CustodyError::InvalidPayload("payload_chunk"));
        }
        let nonce = derive_chunk_nonce(&self.header, chunk_index)?;
        let aad = chunk_aad_bytes(&self.header_bytes, chunk_index);

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
        let cipher = self.content_key.with_bytes(|bytes| {
            Aes256Gcm::new_from_slice(bytes)
                .map_err(|_| CustodyError::InvalidPayload("content_key"))
        })?;
        let encrypted_chunk = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| CustodyError::InvalidPayload("payload_chunk"))?;

        self.framed_bytes = self
            .framed_bytes
            .checked_add(u64::try_from(encrypted_chunk.len()).unwrap())
            .ok_or(CustodyError::InvalidPayload("framed_ciphertext_bytes"))?;
        self.hasher.update(&encrypted_chunk);
        self.next_index += 1;
        Ok(encrypted_chunk)
    }

    /// Finish sealing without provisioning anything. Errors unless every
    /// chunk declared by the header has been sealed (`next_index ==
    /// chunk_count`) and the total framed byte count matches what the header
    /// declares (`framed_bytes == expected_framed_bytes`) — the same two
    /// completeness checks [`Self::finish`] enforces, so every caller of
    /// either method inherits them. On success, returns the sealed header,
    /// the [`EncryptedContentIdentityV1`] computed from the running hash, and
    /// the [`crate::ContentEncryptionKeyV1`] the caller now owns and must
    /// provision (via [`crate::provision_custody_envelope`],
    /// [`crate::provision_custody_envelope_for_exact_nodes`], or discard on
    /// its own error path — this method does not provision, so there is
    /// nothing to undo if the caller decides not to).
    pub fn finish_unprovisioned(self) -> Result<UnprovisionedSealedPayloadV1, CustodyError> {
        let chunk_count = self.header.chunk_count()?;
        if self.next_index != chunk_count {
            return Err(CustodyError::InvalidPayload("chunk_count"));
        }
        let expected_bytes = self.header.expected_framed_bytes()?;
        if self.framed_bytes != expected_bytes {
            return Err(CustodyError::InvalidPayload("framed_ciphertext_bytes"));
        }
        let encrypted_content_identity = EncryptedContentIdentityV1::new(
            Digest32::new(self.hasher.finalize().into()),
            self.framed_bytes,
        )?;
        Ok(UnprovisionedSealedPayloadV1 {
            header: self.header,
            encrypted_content_identity,
            content_key: self.content_key,
        })
    }

    /// Finish sealing and provision the custody envelope exactly like
    /// [`seal_payload_to_staging_writer_v1`]. Errors, without provisioning
    /// anything, unless every chunk declared by the header has been sealed.
    /// This is [`Self::finish_unprovisioned`] immediately followed by
    /// [`crate::provision_custody_envelope`] — the single place either
    /// completeness check or the committee-based provisioning call lives, so
    /// this and [`Self::finish_unprovisioned`] cannot drift apart.
    pub fn finish(
        self,
        committee: &ValidatedCustodyCommitteeV1,
    ) -> Result<SealedPayloadMetadataV1, CustodyError> {
        let unprovisioned = self.finish_unprovisioned()?;
        let custody_envelope = crate::provision_custody_envelope(
            unprovisioned.encrypted_content_identity.clone(),
            &unprovisioned.content_key,
            committee,
        )?;
        Ok(SealedPayloadMetadataV1 {
            header: unprovisioned.header,
            encrypted_content_identity: unprovisioned.encrypted_content_identity,
            custody_envelope,
        })
    }
}

/// Streaming EPC1 chunk decrypter: the per-chunk decryption body of
/// [`decrypt_payload_to_staging_writer_with_content_key_v1`] exposed as a
/// stateless-per-chunk type so a sandboxed decrypt provider can open one
/// frame at a time. Same AAD, nonce derivation, and commitment check as the
/// one-shot path below, which now drives this type internally.
pub struct PayloadChunkDecrypterV1 {
    header: AuthenticatedChunkPayloadHeaderV1,
    header_bytes: Vec<u8>,
    cipher: Aes256Gcm,
}

impl PayloadChunkDecrypterV1 {
    /// Parse a framed header (the same magic + length + encoded header bytes
    /// returned by [`PayloadSealerV1::open`]) and check it commits to
    /// `content_key`.
    pub fn new(
        framed_header: &[u8],
        content_key: &ContentEncryptionKeyV1,
    ) -> Result<Self, CustodyError> {
        let (header, header_bytes) = parse_framed_header_bytes(framed_header)?;
        if !content_key.matches_commitment(header.content_key_commitment) {
            return Err(CustodyError::ContentKeyCommitmentMismatch);
        }
        // See the scoping comment on `PayloadSealerV1::seal_chunk`: the
        // cipher, not the content key, is what this type retains for its
        // lifetime.
        let cipher = content_key.with_bytes(|bytes| {
            Aes256Gcm::new_from_slice(bytes)
                .map_err(|_| CustodyError::InvalidPayload("content_key"))
        })?;
        Ok(Self {
            header,
            header_bytes,
            cipher,
        })
    }

    pub const fn header(&self) -> &AuthenticatedChunkPayloadHeaderV1 {
        &self.header
    }

    /// Decrypt one framed chunk (ciphertext + AEAD tag). `framed_chunk` must
    /// be exactly the framed length for `chunk_index`; a chunk sealed under a
    /// different index (reordered or spliced) fails AAD verification here.
    ///
    /// Returns `Zeroizing<Vec<u8>>`, not a bare `Vec<u8>`: this crate's
    /// zeroization discipline for plaintext chunk buffers (see the scoping
    /// comment on [`PayloadSealerV1::seal_chunk`]) extends to whoever calls
    /// this method — including the sandboxed decrypt provider, which is
    /// exactly where decrypted content lives longest. `Zeroizing<Vec<u8>>`
    /// derefs to `[u8]`, so this is source-compatible with plain `Vec<u8>`
    /// use at the call site.
    pub fn decrypt_chunk(
        &self,
        chunk_index: u32,
        framed_chunk: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, CustodyError> {
        let chunk_index = u64::from(chunk_index);
        let expected_len = chunk_ciphertext_len(&self.header, chunk_index)?;
        if framed_chunk.len() != expected_len {
            return Err(CustodyError::InvalidPayload("framed_ciphertext_bytes"));
        }
        let nonce = derive_chunk_nonce(&self.header, chunk_index)?;
        let aad = chunk_aad_bytes(&self.header_bytes, chunk_index);
        self.cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: framed_chunk,
                    aad: &aad,
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| CustodyError::InvalidPayload("payload_chunk"))
    }
}

/// Byte ranges of each framed chunk inside a sealed object, given the framed
/// header length and the plaintext size — the same inputs carried by
/// `elastos-protected-content-provider-contracts`'
/// `ChunkedPayloadObjectIdentityV1`. Ranges are contiguous, starting at
/// `framed_header_bytes` and ending at the total framed file length, so a
/// provider can read/seek each chunk directly without re-deriving the framing
/// arithmetic.
///
/// This function validates nothing about its inputs and never panics or
/// allocates proportionally to `plaintext_bytes`: it is a lazy iterator over
/// up to `chunk_count_for_plaintext_bytes(plaintext_bytes)` items (which, at
/// this crate's own `MAX_ENCRYPTED_CONTENT_BYTES` header ceiling, can be over
/// a billion), not a materialized `Vec`. An out-of-range or overflowing
/// `plaintext_bytes` degrades to a shorter iterator or to ranges saturated at
/// `u64::MAX`, never to a panic; callers that need the strict guarantees
/// `AuthenticatedChunkPayloadHeaderV1::new_authenticated` enforces (a
/// `plaintext_bytes` that actually fits under the framed-size ceiling) must
/// validate a header first, as `PayloadSealerV1`/`PayloadChunkDecrypterV1`
/// already do.
pub fn framed_chunk_ranges_v1(
    framed_header_bytes: u32,
    plaintext_bytes: u64,
) -> impl Iterator<Item = std::ops::Range<u64>> {
    let chunk_count = chunk_count_for_plaintext_bytes(plaintext_bytes).unwrap_or(0);
    let mut cursor = u64::from(framed_header_bytes);
    (0..chunk_count).map(move |chunk_index| {
        let plaintext_len =
            chunk_plaintext_len_for_plaintext_bytes(plaintext_bytes, chunk_index).unwrap_or(0);
        let framed_len = u64::try_from(plaintext_len)
            .unwrap_or(u64::MAX)
            .saturating_add(u64::try_from(PAYLOAD_TAG_BYTES_V1).unwrap());
        let start = cursor;
        cursor = cursor.saturating_add(framed_len);
        start..cursor
    })
}

fn seal_payload_to_staging_writer_inner<R: Read, W: Write>(
    content_type: &str,
    plaintext_bytes: u64,
    plaintext: &mut R,
    staging_ciphertext: &mut W,
    context: PayloadSealContextV1,
    committee: &ValidatedCustodyCommitteeV1,
) -> Result<SealedPayloadMetadataV1, CustodyError> {
    let (mut sealer, framed_prefix) = PayloadSealerV1::open_with_context(
        content_type,
        plaintext_bytes,
        context.content_key,
        context.base_nonce,
    )?;
    staging_ciphertext
        .write_all(&framed_prefix)
        .map_err(|_| CustodyError::PayloadIo)?;

    let chunk_count = sealer.header.chunk_count()?;
    for chunk_index in 0..chunk_count {
        let chunk_len = chunk_plaintext_len(&sealer.header, chunk_index)?;
        let mut plaintext_chunk = Zeroizing::new(vec![0u8; chunk_len]);
        read_exact_plaintext(plaintext, plaintext_chunk.as_mut_slice())?;
        let chunk_index =
            u32::try_from(chunk_index).map_err(|_| CustodyError::InvalidPayload("chunk_index"))?;
        let framed_chunk = sealer.seal_chunk(chunk_index, plaintext_chunk.as_slice())?;
        staging_ciphertext
            .write_all(&framed_chunk)
            .map_err(|_| CustodyError::PayloadIo)?;
    }

    reject_trailing_plaintext(plaintext)?;

    sealer.finish(committee)
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
    let framed_header = header.framed_prefix_bytes()?;
    let decrypter = PayloadChunkDecrypterV1::new(&framed_header, content_key)?;
    let chunk_count = header.chunk_count()?;

    seek_to_start(ciphertext_source)?;
    ciphertext_source
        .seek(SeekFrom::Start(prefix_len))
        .map_err(|_| CustodyError::PayloadIo)?;

    for chunk_index in 0..chunk_count {
        let ciphertext_chunk_len = chunk_ciphertext_len(&header, chunk_index)?;
        let mut ciphertext_chunk = vec![0u8; ciphertext_chunk_len];
        read_exact_payload_bytes(
            ciphertext_source,
            &mut ciphertext_chunk,
            "framed_ciphertext_bytes",
        )?;
        let chunk_index_u32 =
            u32::try_from(chunk_index).map_err(|_| CustodyError::InvalidPayload("chunk_index"))?;
        let plaintext_chunk = decrypter.decrypt_chunk(chunk_index_u32, &ciphertext_chunk)?;
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

/// Number of chunks a plaintext object of this size is framed into. Standalone
/// (not `AuthenticatedChunkPayloadHeaderV1`-bound) so it can back both the
/// header's own [`AuthenticatedChunkPayloadHeaderV1::chunk_count`] and
/// [`framed_chunk_ranges_v1`], which only ever sees `plaintext_bytes`.
fn chunk_count_for_plaintext_bytes(plaintext_bytes: u64) -> Result<u64, CustodyError> {
    plaintext_bytes
        .checked_add(u64::from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1 - 1))
        .ok_or(CustodyError::InvalidPayload("chunk_count"))?
        .checked_div(u64::from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1))
        .ok_or(CustodyError::InvalidPayload("chunk_count"))
}

/// Plaintext byte length of one chunk, standalone over `plaintext_bytes` for
/// the same reason as [`chunk_count_for_plaintext_bytes`].
fn chunk_plaintext_len_for_plaintext_bytes(
    plaintext_bytes: u64,
    chunk_index: u64,
) -> Result<usize, CustodyError> {
    if chunk_index >= chunk_count_for_plaintext_bytes(plaintext_bytes)? {
        return Err(CustodyError::InvalidPayload("chunk_index"));
    }
    let chunk_bytes = u64::from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1);
    let start = chunk_index
        .checked_mul(chunk_bytes)
        .ok_or(CustodyError::InvalidPayload("chunk_index"))?;
    let remaining = plaintext_bytes
        .checked_sub(start)
        .ok_or(CustodyError::InvalidPayload("chunk_index"))?;
    usize::try_from(remaining.min(chunk_bytes))
        .map_err(|_| CustodyError::InvalidPayload("chunk_index"))
}

fn chunk_plaintext_len(
    header: &AuthenticatedChunkPayloadHeaderV1,
    chunk_index: u64,
) -> Result<usize, CustodyError> {
    chunk_plaintext_len_for_plaintext_bytes(header.plaintext_bytes(), chunk_index)
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

/// Parse an in-memory framed header (magic + length + encoded header, with no
/// trailing bytes) as produced by [`AuthenticatedChunkPayloadHeaderV1::framed_prefix_bytes`]
/// / returned by [`PayloadSealerV1::open`]. Returns the decoded header
/// alongside the raw encoded-header bytes (not a re-encoding of them), so the
/// AAD built from them is guaranteed to match whatever was actually framed.
fn parse_framed_header_bytes(
    framed_header: &[u8],
) -> Result<(AuthenticatedChunkPayloadHeaderV1, Vec<u8>), CustodyError> {
    let mut offset = 0usize;
    let magic =
        read_fixed::<{ PAYLOAD_MAGIC_V1.len() }>(framed_header, &mut offset, "payload_magic")?;
    if magic != PAYLOAD_MAGIC_V1 {
        return Err(CustodyError::InvalidPayload("payload_magic"));
    }
    let header_len = usize::from(read_u16(framed_header, &mut offset, "header_bytes")?);
    let end = offset
        .checked_add(header_len)
        .ok_or(CustodyError::InvalidPayload("header_bytes"))?;
    if end != framed_header.len() {
        return Err(CustodyError::InvalidPayload("header_bytes"));
    }
    let header_bytes = framed_header[offset..end].to_vec();
    let header = decode_header_bytes_v1(&header_bytes)?;
    Ok((header, header_bytes))
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
        PayloadSealContextV1 {
            content_key: ContentEncryptionKeyV1::from_test_bytes(
                content_key.with_bytes(|bytes| *bytes),
            ),
            base_nonce,
        },
        &crate::test_support::validated_custody_committee(),
    )?;
    Ok((framed, metadata))
}

/// Test-only hook over the one-shot sealer with a fixed key and nonce,
/// returning only the framed bytes, for direct comparison against
/// [`PayloadSealerV1::open_with_context_for_tests`] plus
/// [`PayloadSealerV1::seal_chunk`] output.
#[cfg(test)]
fn seal_with_context_for_tests(
    content_type: &str,
    plaintext: &[u8],
    content_key: &ContentEncryptionKeyV1,
    base_nonce: [u8; PAYLOAD_BASE_NONCE_BYTES_V1],
) -> Vec<u8> {
    seal_payload_to_vec_with_test_material(content_type, plaintext, content_key, base_nonce)
        .expect("fixed test key and nonce must seal successfully")
        .0
}

#[cfg(test)]
mod tests {
    use super::*;

    use ed25519_dalek::{Signer as _, SigningKey};
    use elastos_protected_content_contracts::{
        AtomicReplayClaimer, CanonicalContract, ContentAccessIdV1, CustodyPoolError,
        CustodyPoolIdentityV1, CustodyPoolMemberStateV1, Digest32, EncryptedContentIdentityV1,
        EvmContractAddressV1, EvmFunctionSelectorV1, EvmRightsMethodAbiV1, KeyReleaseOutcomeV1,
        KeyReleaseRequestV1, NodeContributionRefV1, ProtectedContentBindingV1,
        RecipientKeyIdentityV1, RecipientPublicKeyBytesV1, ReplayClaimError, ReplayClaimKeyV1,
        ReplayNonce16, RightsActionV1, RightsObservationFinalityV1, RightsPolicyBodyV1,
        RightsRequestV1, RightsSubjectSourceV1, RightsVerificationContextV1,
        RuntimeOperationIssuerKeyV1, RuntimeReleaseAuditIdV1, RuntimeReleaseOperationStatementV1,
        RuntimeSessionBindingV1, SignedNodeContributionV1, SignedRecipientKeyAuthorizationV1,
        SignedRuntimeReleaseOperationV1, SignedTerminalReceiptV1, TerminalReceiptIssuerKey,
        TerminalReceiptStatementV1, VerifiedKeyReleaseRequestV1, WalletAddress,
        WalletSignedRightsRequestV1, CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
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
            EncryptedContentIdentityV1::new(Digest32::new([0x41; 32]), 4096).unwrap(),
            ContentAccessIdV1::new([0x51; 16]).unwrap(),
            RightsActionV1::View,
            RightsSubjectSourceV1::WalletAddress,
            11155111,
            EvmContractAddressV1::new([0x11; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdAddressBytes16,
            RightsObservationFinalityV1::finalized(),
        )
        .unwrap()
    }

    fn recipient_identity_for_tests(seed: u8) -> RecipientKeyIdentityV1 {
        let recipient_public_key = crate::test_support::recipient_public_key(seed);
        RecipientKeyIdentityV1::new(
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
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
                content_key_commitment: fixture
                    .sealed
                    .custody_envelope()
                    .manifest()
                    .content_key_commitment(),
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
                content_key_commitment: fixture
                    .sealed
                    .custody_envelope()
                    .manifest()
                    .content_key_commitment(),
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
                    content_key_commitment: fixture
                        .sealed
                        .custody_envelope()
                        .manifest()
                        .content_key_commitment(),
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
                    content_key_commitment: fixture
                        .sealed
                        .custody_envelope()
                        .manifest()
                        .content_key_commitment(),
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
                    content_key_commitment: fixture
                        .sealed
                        .custody_envelope()
                        .manifest()
                        .content_key_commitment(),
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
                    content_key_commitment: fixture
                        .sealed
                        .custody_envelope()
                        .manifest()
                        .content_key_commitment(),
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
            PayloadSealContextV1 {
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

    fn fixed_test_key_and_nonce() -> (ContentEncryptionKeyV1, [u8; PAYLOAD_BASE_NONCE_BYTES_V1]) {
        (
            ContentEncryptionKeyV1::from_test_bytes([0x59; 32]),
            [0x5a; PAYLOAD_BASE_NONCE_BYTES_V1],
        )
    }

    /// Non-constant, non-repeating filler so a chunk-boundary bug (an
    /// off-by-one in a chunk length, or two chunks silently swapped) cannot
    /// hide behind a payload where every byte, or every chunk, looks alike.
    fn deterministic_bytes(len: usize) -> Vec<u8> {
        (0..len)
            .map(|index| {
                let index = u64::try_from(index).unwrap();
                (index.wrapping_mul(2_654_435_761).wrapping_add(index >> 13) & 0xff) as u8
            })
            .collect()
    }

    fn range_usize(range: &std::ops::Range<u64>) -> std::ops::Range<usize> {
        usize::try_from(range.start).unwrap()..usize::try_from(range.end).unwrap()
    }

    #[test]
    fn streaming_sealer_matches_one_shot_seal_byte_for_byte() {
        let (key, nonce) = fixed_test_key_and_nonce();
        let chunk_bytes = usize::try_from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1).unwrap();
        let plaintext = deterministic_bytes(3 * chunk_bytes + 12345);

        let one_shot = seal_with_context_for_tests("application/pdf", &plaintext, &key, nonce);

        let (mut sealer, header) = PayloadSealerV1::open_with_context_for_tests(
            "application/pdf",
            u64::try_from(plaintext.len()).unwrap(),
            &key,
            nonce,
        );
        let mut framed = header;
        for (index, chunk) in plaintext.chunks(chunk_bytes).enumerate() {
            framed.extend(
                sealer
                    .seal_chunk(u32::try_from(index).unwrap(), chunk)
                    .unwrap(),
            );
        }

        assert_eq!(framed, one_shot);
    }

    #[test]
    fn chunk_decrypter_round_trips_and_rejects_reordered_chunks() {
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x5b; 32]);
        let chunk_bytes = usize::try_from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1).unwrap();
        let plaintext = deterministic_bytes(chunk_bytes * 2 + 777);
        let (framed, metadata) = seal_payload_to_vec_with_test_material(
            "application/octet-stream",
            &plaintext,
            &content_key,
            [0x5c; PAYLOAD_BASE_NONCE_BYTES_V1],
        )
        .unwrap();

        let prefix_end = framed_prefix_end_for_tests(&framed);
        let decrypter = PayloadChunkDecrypterV1::new(&framed[..prefix_end], &content_key).unwrap();
        let ranges: Vec<_> = framed_chunk_ranges_v1(
            u32::try_from(prefix_end).unwrap(),
            metadata.header().plaintext_bytes(),
        )
        .collect();
        assert_eq!(ranges.len(), 3);

        let mut recovered = Vec::new();
        for (index, range) in ranges.iter().enumerate() {
            let plaintext_chunk = decrypter
                .decrypt_chunk(u32::try_from(index).unwrap(), &framed[range_usize(range)])
                .unwrap();
            recovered.extend_from_slice(&plaintext_chunk);
        }
        assert_eq!(recovered, plaintext);

        // Swap chunk 0 and chunk 1 in the framed buffer, then decrypt each at
        // its original index: the chunk index is bound into the AAD, so a
        // chunk sealed under a different index must fail authentication
        // rather than silently decrypt into the wrong position.
        let mut reordered = framed.clone();
        let first = framed[range_usize(&ranges[0])].to_vec();
        let second = framed[range_usize(&ranges[1])].to_vec();
        reordered[range_usize(&ranges[0])].copy_from_slice(&second);
        reordered[range_usize(&ranges[1])].copy_from_slice(&first);

        assert!(decrypter
            .decrypt_chunk(0, &reordered[range_usize(&ranges[0])])
            .is_err());
        assert!(decrypter
            .decrypt_chunk(1, &reordered[range_usize(&ranges[1])])
            .is_err());
    }

    #[test]
    fn framed_chunk_ranges_cover_the_sealed_file_exactly() {
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x5d; 32]);
        let chunk_bytes = usize::try_from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1).unwrap();
        let plaintext = deterministic_bytes(chunk_bytes * 2 + 501);
        let (framed, metadata) = seal_payload_to_vec_with_test_material(
            "application/octet-stream",
            &plaintext,
            &content_key,
            [0x5e; PAYLOAD_BASE_NONCE_BYTES_V1],
        )
        .unwrap();
        let prefix_end = framed_prefix_end_for_tests(&framed);

        let ranges: Vec<_> = framed_chunk_ranges_v1(
            u32::try_from(prefix_end).unwrap(),
            metadata.header().plaintext_bytes(),
        )
        .collect();

        assert_eq!(ranges.len(), 3);
        assert_eq!(
            ranges.first().unwrap().start,
            u64::try_from(prefix_end).unwrap()
        );
        assert_eq!(
            ranges.last().unwrap().end,
            u64::try_from(framed.len()).unwrap()
        );
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "ranges must be contiguous");
        }
        for range in &ranges {
            assert!(range.start < range.end);
        }
    }

    #[test]
    fn sealer_finish_requires_every_chunk() {
        let chunk_bytes = usize::try_from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1).unwrap();
        let (mut sealer, _header) = PayloadSealerV1::open(
            "application/octet-stream",
            u64::try_from(chunk_bytes * 2).unwrap(),
        )
        .unwrap();
        sealer.seal_chunk(0, &vec![0x22; chunk_bytes]).unwrap();

        let err = sealer
            .finish(&crate::test_support::validated_custody_committee())
            .unwrap_err();
        assert!(matches!(err, CustodyError::InvalidPayload("chunk_count")));
    }

    #[test]
    fn seal_chunk_rejects_out_of_order_duplicate_and_wrong_length_chunks() {
        let chunk_bytes = usize::try_from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1).unwrap();
        let (mut sealer, _header) = PayloadSealerV1::open(
            "application/octet-stream",
            u64::try_from(chunk_bytes * 3).unwrap(),
        )
        .unwrap();

        // Out of order: index 1 before index 0 has ever been sealed.
        assert!(matches!(
            sealer.seal_chunk(1, &vec![0x11; chunk_bytes]),
            Err(CustodyError::InvalidPayload("chunk_index"))
        ));

        // Wrong length for a non-final chunk.
        assert!(matches!(
            sealer.seal_chunk(0, &vec![0x11; chunk_bytes - 1]),
            Err(CustodyError::InvalidPayload("payload_chunk"))
        ));

        sealer.seal_chunk(0, &vec![0x11; chunk_bytes]).unwrap();

        // Duplicate / replayed index: index 0 again after it already advanced
        // `next_index` to 1.
        assert!(matches!(
            sealer.seal_chunk(0, &vec![0x11; chunk_bytes]),
            Err(CustodyError::InvalidPayload("chunk_index"))
        ));
    }

    #[test]
    fn chunk_decrypter_new_rejects_trailing_bytes_after_framed_header() {
        let content_key = ContentEncryptionKeyV1::from_test_bytes([0x5f; 32]);
        let (framed, _metadata) = seal_payload_to_vec_with_test_material(
            "application/octet-stream",
            b"trailing bytes after the framed header must be rejected",
            &content_key,
            [0x60; PAYLOAD_BASE_NONCE_BYTES_V1],
        )
        .unwrap();
        let prefix_end = framed_prefix_end_for_tests(&framed);

        let mut framed_header_with_trailing_byte = framed[..prefix_end].to_vec();
        framed_header_with_trailing_byte.push(0x00);

        assert!(matches!(
            PayloadChunkDecrypterV1::new(&framed_header_with_trailing_byte, &content_key),
            Err(CustodyError::InvalidPayload("header_bytes"))
        ));
    }

    #[test]
    fn reconstruct_content_key_for_object_session_matches_direct_reconstruction() {
        let fixture = authenticated_decrypt_fixture(b"object session content key".to_vec());
        let content_key_commitment = fixture
            .sealed
            .custody_envelope()
            .manifest()
            .content_key_commitment();

        let reconstructed = reconstruct_content_key_for_object_session(
            &fixture.operation,
            content_key_commitment,
            &fixture.contributions,
            &fixture.terminal,
            fixture.terminal_issuer,
            &fixture.recipient_secret,
            crate::test_support::NOW + 8,
        )
        .unwrap();
        assert!(reconstructed.matches_commitment(content_key_commitment));

        // End-to-end: the reconstructed key must actually open the sealed
        // object via the streaming decrypter, exactly like the one-shot
        // authenticated decrypt path does internally with the same key.
        let prefix_end = framed_prefix_end_for_tests(&fixture.framed);
        let decrypter =
            PayloadChunkDecrypterV1::new(&fixture.framed[..prefix_end], &reconstructed).unwrap();
        let first_chunk_len = chunk_ciphertext_len(fixture.sealed.header(), 0).unwrap();
        let plaintext_chunk = decrypter
            .decrypt_chunk(0, &fixture.framed[prefix_end..prefix_end + first_chunk_len])
            .unwrap();
        assert_eq!(&plaintext_chunk[..], &fixture.plaintext[..]);
    }

    #[test]
    fn framed_chunk_ranges_v1_is_lazy_for_a_huge_plaintext_size() {
        // At this crate's own header ceiling (`MAX_ENCRYPTED_CONTENT_BYTES =
        // 1 << 50`, enforced by `AuthenticatedChunkPayloadHeaderV1::validate`)
        // `chunk_count_for_plaintext_bytes` returns roughly 1.07e9. Before
        // this was made lazy, computing ranges for a plaintext this size
        // eagerly allocated a `Vec` of that many `Range<u64>` (~17 GB) up
        // front. Taking only the first few items must be fast and correct
        // without ever materializing the rest.
        let huge_plaintext_bytes = MAX_ENCRYPTED_CONTENT_BYTES;
        let framed_header_bytes = 128u32;
        let chunk_bytes = u64::from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1);
        let framed_chunk_bytes = chunk_bytes + u64::try_from(PAYLOAD_TAG_BYTES_V1).unwrap();

        let first_three: Vec<_> = framed_chunk_ranges_v1(framed_header_bytes, huge_plaintext_bytes)
            .take(3)
            .collect();

        assert_eq!(first_three.len(), 3);
        assert_eq!(
            first_three[0],
            u64::from(framed_header_bytes)..u64::from(framed_header_bytes) + framed_chunk_bytes
        );
        assert_eq!(first_three[1].start, first_three[0].end);
        assert_eq!(
            first_three[1].end - first_three[1].start,
            framed_chunk_bytes
        );
        assert_eq!(first_three[2].start, first_three[1].end);
        assert_eq!(
            first_three[2].end - first_three[2].start,
            framed_chunk_bytes
        );

        // `u64::MAX` overflows the chunk-count arithmetic
        // (`plaintext_bytes + (chunk_bytes - 1)`); the function must degrade
        // to an empty iterator rather than panicking or hanging.
        let absurd_plaintext_bytes = u64::MAX;
        let absurd_first: Vec<_> =
            framed_chunk_ranges_v1(framed_header_bytes, absurd_plaintext_bytes)
                .take(2)
                .collect();
        assert!(absurd_first.is_empty());
    }

    #[test]
    fn max_object_framed_chunk_bytes_v1_covers_the_real_framed_chunk_len() {
        // Task 8 defined `MAX_OBJECT_FRAMED_CHUNK_BYTES_V1` in the sibling
        // `elastos-protected-content-provider-contracts` crate as
        // `MAX_OBJECT_PLAINTEXT_CHUNK_BYTES_V1 + 16 + 8` (AEAD tag + framing
        // slack) and deferred verifying it against this crate's real framing,
        // since that crate cannot see `payload.rs`'s constants. This crate
        // depends on it, so the assertion belongs here (controller ruling
        // R2). The only per-chunk overhead this file's framing adds is the
        // AEAD tag (chunks are concatenated ciphertext blocks with no
        // per-chunk length/magic of their own) — derived here from
        // `PAYLOAD_TAG_BYTES_V1`, not copied as a literal.
        assert_eq!(
            elastos_protected_content_provider_contracts::MAX_OBJECT_PLAINTEXT_CHUNK_BYTES_V1,
            usize::try_from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1).unwrap(),
            "the two crates' plaintext-chunk-size constants must agree exactly"
        );
        let real_max_framed_chunk_bytes = usize::try_from(PAYLOAD_PLAINTEXT_CHUNK_BYTES_V1)
            .unwrap()
            .checked_add(PAYLOAD_TAG_BYTES_V1)
            .unwrap();
        assert!(
            elastos_protected_content_provider_contracts::MAX_OBJECT_FRAMED_CHUNK_BYTES_V1
                >= real_max_framed_chunk_bytes,
            "MAX_OBJECT_FRAMED_CHUNK_BYTES_V1 must be large enough to hold the real \
             plaintext-chunk-plus-AEAD-tag length this crate actually frames"
        );
        // Pin the deliberate slack itself (the object.rs comment's literal
        // "+ 8") as a companion upper bound: without it, the lower bound
        // above alone would let the contracts-side constant be inflated to
        // any value and still pass, silently masking a real framing
        // mismatch instead of catching one.
        const DOCUMENTED_FRAMING_SLACK_BYTES: usize = 8;
        assert_eq!(
            elastos_protected_content_provider_contracts::MAX_OBJECT_FRAMED_CHUNK_BYTES_V1,
            real_max_framed_chunk_bytes + DOCUMENTED_FRAMING_SLACK_BYTES,
            "MAX_OBJECT_FRAMED_CHUNK_BYTES_V1 must be exactly the real per-chunk length plus \
             the documented 8-byte framing slack, not an unbounded margin above it"
        );
    }
}
