//! Source-only X-Wing draft-06 share wrapping.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use hkdf::Hkdf;
use hpke::rand_core::{CryptoRng, RngCore};
use sha2::Sha256;
use x_wing::kem::{Decapsulate as _, Decapsulator as _, KeyExport as _};
use x_wing::TryKeyInit as _;
use zeroize::Zeroizing;

use elastos_protected_content_contracts::{
    NodeCustodyPublicKeyV1, CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
    PQ_HYBRID_SEALED_SHARE_ENVELOPE_BYTES, PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES,
    X_WING_DRAFT06_CIPHERTEXT_BYTES,
};

pub const SUITE_PQ_HYBRID: &str = CUSTODY_X_WING_AES256GCM_SUITE_ID_V1;
const HKDF_DOMAIN_V1: &[u8] = b"elastos.protected-content.xwing-draft06.hkdf-sha256/v1";
const PQ_HYBRID_AEAD_NONCE_BYTES: usize = 12;
const PQ_HYBRID_WRAPPED_SHARE_BYTES: usize = 48;
pub const X_WING_ENCAPSULATION_RANDOMNESS_BYTES: usize = x_wing::ENCAPSULATION_RANDOMNESS_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PqHybridError {
    InvalidPublicKey,
    InvalidCiphertext,
    DecapFailed,
    SealFailed,
    UnsealFailed,
}

impl std::fmt::Display for PqHybridError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPublicKey => formatter.write_str("invalid x-wing public key"),
            Self::InvalidCiphertext => formatter.write_str("invalid x-wing ciphertext"),
            Self::DecapFailed => formatter.write_str("x-wing decapsulation failed"),
            Self::SealFailed => formatter.write_str("x-wing seal failed"),
            Self::UnsealFailed => formatter.write_str("x-wing unseal failed"),
        }
    }
}

impl std::error::Error for PqHybridError {}

pub struct SessionKemSecret {
    secret: x_wing::DecapsulationKey,
}

pub struct SessionKemPublic {
    public: x_wing::EncapsulationKey,
}

#[derive(Clone, PartialEq, Eq)]
pub struct PqSealedEnvelope {
    ciphertext: [u8; X_WING_DRAFT06_CIPHERTEXT_BYTES],
    nonce: [u8; PQ_HYBRID_AEAD_NONCE_BYTES],
    wrapped_share: [u8; PQ_HYBRID_WRAPPED_SHARE_BYTES],
}

impl std::fmt::Debug for PqSealedEnvelope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PqSealedEnvelope([redacted])")
    }
}

pub(crate) fn session_secret_from_bytes(secret_key_bytes: [u8; 32]) -> SessionKemSecret {
    SessionKemSecret {
        secret: x_wing::DecapsulationKey::from(secret_key_bytes),
    }
}

pub(crate) fn session_public_bytes_from_secret_bytes(
    secret_key_bytes: [u8; 32],
) -> [u8; PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES] {
    let secret = x_wing::DecapsulationKey::from(secret_key_bytes);
    secret.encapsulation_key().to_bytes().into()
}

pub(crate) fn session_public_from_bytes(bytes: &[u8]) -> Result<SessionKemPublic, PqHybridError> {
    if bytes.len() != PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES {
        return Err(PqHybridError::InvalidPublicKey);
    }
    let array: [u8; PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES] = bytes
        .try_into()
        .map_err(|_| PqHybridError::InvalidPublicKey)?;
    NodeCustodyPublicKeyV1::new(array).map_err(|_| PqHybridError::InvalidPublicKey)?;
    let public = x_wing::EncapsulationKey::new_from_slice(bytes)
        .map_err(|_| PqHybridError::InvalidPublicKey)?;
    Ok(SessionKemPublic { public })
}

#[cfg(test)]
pub(crate) fn mint_session_from_seed(seed: [u8; 32]) -> (SessionKemSecret, SessionKemPublic) {
    let secret = x_wing::DecapsulationKey::from(seed);
    let public = secret.encapsulation_key().clone();
    (SessionKemSecret { secret }, SessionKemPublic { public })
}

fn derive_wrap_key(shared_secret: &[u8], aad: &[u8]) -> Result<Zeroizing<[u8; 32]>, PqHybridError> {
    let mut info = Vec::with_capacity(
        HKDF_DOMAIN_V1.len() + 2 + CUSTODY_X_WING_AES256GCM_SUITE_ID_V1.len() + 4 + aad.len(),
    );
    info.extend_from_slice(HKDF_DOMAIN_V1);
    info.extend_from_slice(&(CUSTODY_X_WING_AES256GCM_SUITE_ID_V1.len() as u16).to_be_bytes());
    info.extend_from_slice(CUSTODY_X_WING_AES256GCM_SUITE_ID_V1.as_bytes());
    info.extend_from_slice(&(aad.len() as u32).to_be_bytes());
    info.extend_from_slice(aad);

    let hkdf = Hkdf::<Sha256>::new(None, shared_secret);
    let mut key = Zeroizing::new([0u8; 32]);
    hkdf.expand(&info, &mut *key)
        .map_err(|_| PqHybridError::SealFailed)?;
    Ok(key)
}

impl PqSealedEnvelope {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(PQ_HYBRID_SEALED_SHARE_ENVELOPE_BYTES);
        bytes.extend_from_slice(&self.ciphertext);
        bytes.extend_from_slice(&self.nonce);
        bytes.extend_from_slice(&self.wrapped_share);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PqHybridError> {
        if bytes.len() != PQ_HYBRID_SEALED_SHARE_ENVELOPE_BYTES {
            return Err(PqHybridError::InvalidCiphertext);
        }
        let ciphertext = bytes[..X_WING_DRAFT06_CIPHERTEXT_BYTES]
            .try_into()
            .map_err(|_| PqHybridError::InvalidCiphertext)?;
        let nonce = bytes[X_WING_DRAFT06_CIPHERTEXT_BYTES
            ..X_WING_DRAFT06_CIPHERTEXT_BYTES + PQ_HYBRID_AEAD_NONCE_BYTES]
            .try_into()
            .map_err(|_| PqHybridError::InvalidCiphertext)?;
        let wrapped_share = bytes[X_WING_DRAFT06_CIPHERTEXT_BYTES + PQ_HYBRID_AEAD_NONCE_BYTES..]
            .try_into()
            .map_err(|_| PqHybridError::InvalidCiphertext)?;
        Ok(Self {
            ciphertext,
            nonce,
            wrapped_share,
        })
    }
}

#[cfg(test)]
fn seal_bound_inner(
    public: &SessionKemPublic,
    share: &[u8],
    aad: &[u8],
    nonce: [u8; PQ_HYBRID_AEAD_NONCE_BYTES],
    encapsulation_randomness: [u8; X_WING_ENCAPSULATION_RANDOMNESS_BYTES],
) -> Result<PqSealedEnvelope, PqHybridError> {
    let (ciphertext, shared_secret) = public
        .public
        .encapsulate_deterministic(&encapsulation_randomness.into());
    let ciphertext: [u8; X_WING_DRAFT06_CIPHERTEXT_BYTES] = ciphertext.into();
    let wrap_key = derive_wrap_key(shared_secret.as_slice(), aad)?;
    let cipher =
        Aes256Gcm::new_from_slice(wrap_key.as_slice()).map_err(|_| PqHybridError::SealFailed)?;
    let wrapped_share = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: share, aad })
        .map_err(|_| PqHybridError::SealFailed)?;
    let wrapped_share: [u8; PQ_HYBRID_WRAPPED_SHARE_BYTES] = wrapped_share
        .try_into()
        .map_err(|_| PqHybridError::SealFailed)?;
    Ok(PqSealedEnvelope {
        ciphertext,
        nonce,
        wrapped_share,
    })
}

pub(crate) fn seal_bound<R: CryptoRng + RngCore>(
    public: &SessionKemPublic,
    share: &[u8],
    aad: &[u8],
    rng: &mut R,
) -> Result<PqSealedEnvelope, PqHybridError> {
    let mut nonce = [0u8; PQ_HYBRID_AEAD_NONCE_BYTES];
    rng.fill_bytes(&mut nonce);
    let mut encapsulation_randomness = Zeroizing::new([0u8; X_WING_ENCAPSULATION_RANDOMNESS_BYTES]);
    rng.fill_bytes(encapsulation_randomness.as_mut_slice());
    let (ciphertext, shared_secret) = public
        .public
        .encapsulate_deterministic((&*encapsulation_randomness).into());
    let ciphertext: [u8; X_WING_DRAFT06_CIPHERTEXT_BYTES] = ciphertext.into();
    let wrap_key = derive_wrap_key(shared_secret.as_slice(), aad)?;
    let cipher =
        Aes256Gcm::new_from_slice(wrap_key.as_slice()).map_err(|_| PqHybridError::SealFailed)?;
    let wrapped_share = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: share, aad })
        .map_err(|_| PqHybridError::SealFailed)?;
    let wrapped_share: [u8; PQ_HYBRID_WRAPPED_SHARE_BYTES] = wrapped_share
        .try_into()
        .map_err(|_| PqHybridError::SealFailed)?;
    Ok(PqSealedEnvelope {
        ciphertext,
        nonce,
        wrapped_share,
    })
}

pub(crate) fn hybrid_unwrap_bound(
    session: &SessionKemSecret,
    envelope: &PqSealedEnvelope,
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>, PqHybridError> {
    let shared_secret = session
        .secret
        .decapsulate_slice(&envelope.ciphertext)
        .map_err(|_| PqHybridError::DecapFailed)?;
    let wrap_key = derive_wrap_key(shared_secret.as_slice(), aad)?;
    let cipher =
        Aes256Gcm::new_from_slice(wrap_key.as_slice()).map_err(|_| PqHybridError::UnsealFailed)?;
    let share = cipher
        .decrypt(
            Nonce::from_slice(&envelope.nonce),
            Payload {
                msg: envelope.wrapped_share.as_ref(),
                aad,
            },
        )
        .map_err(|_| PqHybridError::UnsealFailed)?;
    Ok(Zeroizing::new(share))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest as _, Sha256};

    fn share() -> [u8; 32] {
        [0x42; 32]
    }

    fn valid_public_bytes(seed: u8) -> [u8; PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES] {
        session_public_bytes_from_secret_bytes([seed; 32])
    }

    #[test]
    fn secret_import_export_roundtrip_preserves_public_identity() {
        let secret_bytes = [0x4d; 32];
        let public_bytes = session_public_bytes_from_secret_bytes(secret_bytes);
        let secret = session_secret_from_bytes(secret_bytes);
        assert_eq!(secret.secret.as_bytes(), &secret_bytes);
        assert_eq!(
            session_public_bytes_from_secret_bytes(*secret.secret.as_bytes()),
            public_bytes
        );

        let reimported = session_secret_from_bytes(*secret.secret.as_bytes());
        assert_eq!(
            session_public_bytes_from_secret_bytes(*reimported.secret.as_bytes()),
            public_bytes
        );
    }

    #[test]
    fn session_public_rejects_wrong_length() {
        assert!(matches!(
            session_public_from_bytes(&[0x11; 32]),
            Err(PqHybridError::InvalidPublicKey)
        ));
    }

    #[test]
    fn session_public_rejects_old_wire_order_and_invalid_final_x25519_component() {
        let valid = valid_public_bytes(0x4d);

        let mut old_wire_order = [0u8; PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES];
        let ml_kem_len = valid.len() - 32;
        old_wire_order[..32].copy_from_slice(&valid[ml_kem_len..]);
        old_wire_order[32..].copy_from_slice(&valid[..ml_kem_len]);
        assert!(matches!(
            session_public_from_bytes(&old_wire_order),
            Err(PqHybridError::InvalidPublicKey)
        ));

        let mut low_order = valid;
        let x25519_offset = low_order.len() - 32;
        low_order[x25519_offset..].fill(0);
        low_order[x25519_offset] = 1;
        assert!(matches!(
            session_public_from_bytes(&low_order),
            Err(PqHybridError::InvalidPublicKey)
        ));

        let mut high_bit_alias = valid;
        high_bit_alias[high_bit_alias.len() - 1] |= 0x80;
        assert!(matches!(
            session_public_from_bytes(&high_bit_alias),
            Err(PqHybridError::InvalidPublicKey)
        ));
    }

    #[test]
    fn hybrid_roundtrip_is_deterministic_with_fixed_vector() {
        let (secret, public) = mint_session_from_seed([0x4d; 32]);
        let envelope = seal_bound_inner(
            &public,
            &share(),
            b"transcript-a",
            [0x24; PQ_HYBRID_AEAD_NONCE_BYTES],
            [0x11; X_WING_ENCAPSULATION_RANDOMNESS_BYTES],
        )
        .expect("seal");
        assert_eq!(
            envelope.to_bytes().len(),
            PQ_HYBRID_SEALED_SHARE_ENVELOPE_BYTES
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(envelope.to_bytes())),
            "acc1026fb61f67ac46eb58cf1c66b78b95f03f896b1ab7da0092fd19ba80aca4"
        );
        let opened = hybrid_unwrap_bound(&secret, &envelope, b"transcript-a").expect("unwrap");
        assert_eq!(opened.as_slice(), share());
        assert!(!envelope
            .wrapped_share
            .windows(share().len())
            .any(|window| window == share()));
        assert!(!envelope
            .to_bytes()
            .windows(share().len())
            .any(|window| window == share()));
        let debug = format!("{envelope:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("42"));
    }

    #[test]
    fn hybrid_unwrap_fails_closed_on_wrong_key_or_aad_or_tamper() {
        let (secret, public) = mint_session_from_seed([0x4d; 32]);
        let (wrong_secret, _) = mint_session_from_seed([0x4e; 32]);
        let envelope = seal_bound_inner(
            &public,
            &share(),
            b"transcript-a",
            [0x24; PQ_HYBRID_AEAD_NONCE_BYTES],
            [0x11; X_WING_ENCAPSULATION_RANDOMNESS_BYTES],
        )
        .expect("seal");
        assert_eq!(
            hybrid_unwrap_bound(&wrong_secret, &envelope, b"transcript-a").unwrap_err(),
            PqHybridError::UnsealFailed
        );
        assert_eq!(
            hybrid_unwrap_bound(&secret, &envelope, b"transcript-b").unwrap_err(),
            PqHybridError::UnsealFailed
        );
        let mut tampered = envelope.clone();
        tampered.wrapped_share[0] ^= 0xff;
        assert_eq!(
            hybrid_unwrap_bound(&secret, &tampered, b"transcript-a").unwrap_err(),
            PqHybridError::UnsealFailed
        );
    }

    #[test]
    fn envelope_rejects_truncation_and_extra_bytes() {
        let (_, public) = mint_session_from_seed([0x4d; 32]);
        let envelope = seal_bound_inner(
            &public,
            &share(),
            b"transcript-a",
            [0x24; PQ_HYBRID_AEAD_NONCE_BYTES],
            [0x11; X_WING_ENCAPSULATION_RANDOMNESS_BYTES],
        )
        .expect("seal")
        .to_bytes();
        assert_eq!(
            PqSealedEnvelope::from_bytes(&envelope[..envelope.len() - 1]).unwrap_err(),
            PqHybridError::InvalidCiphertext
        );
        let mut extra = envelope.clone();
        extra.push(0);
        assert_eq!(
            PqSealedEnvelope::from_bytes(&extra).unwrap_err(),
            PqHybridError::InvalidCiphertext
        );
    }
}
