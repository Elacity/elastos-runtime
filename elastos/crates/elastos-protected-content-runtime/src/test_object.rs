//! Deterministic `ChunkedPayloadObjectIdentityV1` fixtures for tests, mirroring
//! `test_media`'s idiom for the non-media (object) content kind.

use elastos_protected_content_contracts::{Digest32, EncryptedContentIdentityV1};
use elastos_protected_content_provider_contracts::ChunkedPayloadObjectIdentityV1;

const CONTENT_TYPE_V1: &str = "application/octet-stream";
const FRAMED_HEADER_BYTES_V1: u32 = 64;
const PLAINTEXT_BYTES_V1: u64 = 4096;

/// A valid object identity seeded from `seed`, with a framed ciphertext large
/// enough to satisfy `ChunkedPayloadObjectIdentityV1`'s
/// header-plus-plaintext invariant.
pub(crate) fn object_identity(seed: u8) -> ChunkedPayloadObjectIdentityV1 {
    let encrypted_content = EncryptedContentIdentityV1::new(
        Digest32::new([seed; 32]),
        PLAINTEXT_BYTES_V1 + u64::from(FRAMED_HEADER_BYTES_V1),
    )
    .unwrap();
    ChunkedPayloadObjectIdentityV1::new(
        encrypted_content,
        CONTENT_TYPE_V1,
        PLAINTEXT_BYTES_V1,
        FRAMED_HEADER_BYTES_V1,
    )
    .unwrap()
}
