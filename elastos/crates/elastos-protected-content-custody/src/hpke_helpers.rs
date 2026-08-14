use hpke::{
    aead::{AeadTag, AesGcm256},
    kdf::HkdfSha256,
    kem::{Kem as KemTrait, X25519HkdfSha256},
    rand_core::{CryptoRng, RngCore},
    single_shot_open_in_place_detached, single_shot_seal_in_place_detached, Deserializable,
    OpModeR, OpModeS, Serializable,
};
use zeroize::Zeroizing;

use elastos_protected_content_contracts::{
    CanonicalContract, HpkeCiphertextV1, NodeCustodyPublicKeyV1, HPKE_SEALED_SHARE_BYTES,
};

use crate::{CustodyError, CONTENT_KEY_BYTES, RELEASED_SHARE_TAG_BYTES};

pub(crate) type HpkeKem = X25519HkdfSha256;
type HpkeAead = AesGcm256;
type HpkeKdf = HkdfSha256;

pub(crate) fn seal_share<R: CryptoRng + RngCore>(
    public_key_bytes: &[u8; 32],
    info: &[u8],
    aad: &[u8],
    plaintext_share: &[u8; CONTENT_KEY_BYTES],
    rng: &mut R,
) -> Result<HpkeCiphertextV1, CustodyError> {
    let public_key = <HpkeKem as KemTrait>::PublicKey::from_bytes(public_key_bytes)?;
    let mut ciphertext = Zeroizing::new([0u8; CONTENT_KEY_BYTES]);
    ciphertext.copy_from_slice(plaintext_share);
    let (encapped_key, tag) = single_shot_seal_in_place_detached::<HpkeAead, HpkeKdf, HpkeKem, _>(
        &OpModeS::Base,
        &public_key,
        info,
        &mut *ciphertext,
        aad,
        rng,
    )?;
    let mut sealed = [0u8; HPKE_SEALED_SHARE_BYTES];
    sealed[..CONTENT_KEY_BYTES].copy_from_slice(&ciphertext[..]);
    sealed[CONTENT_KEY_BYTES..].copy_from_slice(tag.to_bytes().as_slice());
    HpkeCiphertextV1::new(encapped_key.to_bytes().into(), sealed).map_err(Into::into)
}

pub(crate) fn open_share(
    ciphertext: &HpkeCiphertextV1,
    secret_key_bytes: &[u8; 32],
    info: &[u8],
    aad: &[u8],
) -> Result<Zeroizing<[u8; CONTENT_KEY_BYTES]>, CustodyError> {
    NodeCustodyPublicKeyV1::new(*ciphertext.encapped_key())
        .map_err(|_| CustodyError::MalformedShare("hpke_encapped_key"))?;
    let secret_key = <HpkeKem as KemTrait>::PrivateKey::from_bytes(secret_key_bytes)?;
    let encapped_key = <HpkeKem as KemTrait>::EncappedKey::from_bytes(ciphertext.encapped_key())?;
    let tag = AeadTag::<HpkeAead>::from_bytes(&ciphertext.ciphertext()[CONTENT_KEY_BYTES..])?;
    let mut plaintext = Zeroizing::new([0u8; CONTENT_KEY_BYTES]);
    plaintext.copy_from_slice(&ciphertext.ciphertext()[..CONTENT_KEY_BYTES]);
    debug_assert_eq!(RELEASED_SHARE_TAG_BYTES, AeadTag::<HpkeAead>::size());
    single_shot_open_in_place_detached::<HpkeAead, HpkeKdf, HpkeKem>(
        &OpModeR::Base,
        &secret_key,
        &encapped_key,
        info,
        &mut *plaintext,
        aad,
        &tag,
    )?;
    Ok(plaintext)
}

pub(crate) fn decode_ciphertext_bytes(bytes: &[u8]) -> Result<HpkeCiphertextV1, CustodyError> {
    HpkeCiphertextV1::from_canonical_bytes(bytes).map_err(Into::into)
}
