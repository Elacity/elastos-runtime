use hpke::rand_core::{CryptoRng, RngCore};
use zeroize::Zeroizing;

use elastos_protected_content_contracts::{
    CanonicalContract, PqHybridSealedShareV1, PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES,
};

use crate::pq_hybrid::{
    hybrid_unwrap_bound, seal_bound, session_public_from_bytes, session_secret_from_bytes,
    PqSealedEnvelope,
};
use crate::{CustodyError, CONTENT_KEY_BYTES};

fn bound_aad(info: &[u8], aad: &[u8]) -> Vec<u8> {
    let mut bound = Vec::with_capacity(info.len() + aad.len());
    bound.extend_from_slice(info);
    bound.extend_from_slice(aad);
    bound
}

pub(crate) fn seal_share<R: CryptoRng + RngCore>(
    public_key_bytes: &[u8; PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES],
    info: &[u8],
    aad: &[u8],
    plaintext_share: &[u8; CONTENT_KEY_BYTES],
    rng: &mut R,
) -> Result<PqHybridSealedShareV1, CustodyError> {
    let public = session_public_from_bytes(public_key_bytes.as_slice())
        .map_err(|_| CustodyError::BindingMismatch("pq_hybrid_kem"))?;
    let envelope = seal_bound(&public, plaintext_share, &bound_aad(info, aad), rng)?;
    PqHybridSealedShareV1::new(envelope.to_bytes()).map_err(Into::into)
}

pub(crate) fn open_share(
    ciphertext: &PqHybridSealedShareV1,
    secret_key_bytes: &[u8; 32],
    info: &[u8],
    aad: &[u8],
) -> Result<Zeroizing<[u8; CONTENT_KEY_BYTES]>, CustodyError> {
    let envelope = PqSealedEnvelope::from_bytes(ciphertext.envelope())
        .map_err(|_| CustodyError::MalformedShare("pq_hybrid_envelope"))?;
    let secret = session_secret_from_bytes(*secret_key_bytes);
    let opened = hybrid_unwrap_bound(&secret, &envelope, &bound_aad(info, aad))?;
    if opened.len() != CONTENT_KEY_BYTES {
        return Err(CustodyError::MalformedShare("pq_hybrid_share_len"));
    }
    let mut share = Zeroizing::new([0u8; CONTENT_KEY_BYTES]);
    share.copy_from_slice(&opened);
    Ok(share)
}

pub(crate) fn decode_ciphertext_bytes(bytes: &[u8]) -> Result<PqHybridSealedShareV1, CustodyError> {
    PqHybridSealedShareV1::from_canonical_bytes(bytes).map_err(Into::into)
}
