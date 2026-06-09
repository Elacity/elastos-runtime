//! ElastOS dDRM PQ-hybrid CEK-seal envelope — the **single source of truth** for
//! the runtime's shipped decrypt profile (`elastos-pq-hybrid-threshold-v0`).
//!
//! Two boundaries share this exact code so they can never drift:
//!   - the **key authority** (`key-provider` + its backends) *seals* a CEK to a
//!     decrypt session's published key — [`seal::seal_bound`];
//!   - the **decrypt boundary** (`decrypt-provider`) *unwraps* it in-VM —
//!     [`hybrid_unwrap_bound`].
//!
//! Crypto (byte-identical to the proven `decrypt-provider::pq_envelope` island it
//! is extracted from):
//!   - **Hybrid KEM:** `x25519` DH ‖ `ML-KEM-768` (FIPS 203). The AEAD wrap key is
//!     derived from BOTH shared secrets, so confidentiality holds if EITHER
//!     primitive stays unbroken (classical OR post-quantum).
//!   - **AEAD wrap:** `AES-256-GCM` over the CEK — a wrong KEM secret or tampered
//!     blob fails closed (no plaintext on error).
//!   - **Signature:** behind [`CekSealVerifier`] / [`seal::CekSealSigner`] so the
//!     scheme is swappable; the product root is `ml-dsa-65` ([`MlDsa65Verifier`]).
//!   - **Transcript binding:** `*_bound` carries an external `aad` (the canonical
//!     decrypt transcript) into BOTH the AES-256-GCM AAD and the signed payload, so
//!     a CEK sealed for one transcript can never open under another. `aad == b""`
//!     reproduces the unbound wire exactly (committed goldens unchanged).
//!
//! Containment invariants: the CEK materialises only after a correct hybrid-KEM +
//! AEAD open, is returned in `Zeroizing`, never appears in the sealed bytes, and the
//! unwrap path needs NO RNG and NO outbound authority — a pure in-boundary transform.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use ml_kem::kem::Decapsulate;
use ml_kem::KemCore;
use sha2::{Digest, Sha256};
use x25519_dalek::PublicKey as XPublicKey;
use zeroize::Zeroizing;

// Re-exported so the decrypt boundary can name the exact KEM / x25519 types this
// crate's envelope is built from — one provenance, no parallel type definitions.
pub use ml_kem::{Ciphertext, MlKem768};
pub use x25519_dalek::StaticSecret as XStaticSecret;

pub type MlKemDk = <MlKem768 as KemCore>::DecapsulationKey;
pub type MlKemEk = <MlKem768 as KemCore>::EncapsulationKey;

/// Domain separation + profile binding for the wrap-key KDF.
const KDF_LABEL: &[u8] = b"elastos-pq-hybrid-threshold-v0/cek-wrap/v1";

/// The decrypt-material suite tag this envelope implements.
pub const SUITE_PQ_HYBRID: &str = "elastos-pq-hybrid-threshold-v0";

/// Fail-closed error surface. Messages are coarse so a forged envelope cannot probe
/// internal state (which half failed).
#[derive(Debug, PartialEq, Eq)]
pub enum PqEnvelopeError {
    BadSignature,
    DecapFailed,
    UnsealFailed,
}

/// The decrypt VM's per-session hybrid KEM secret. The secret never leaves the VM;
/// `x25519` is zeroized on drop and the ML-KEM decapsulation key holds its secret
/// internally.
pub struct SessionKemSecret {
    pub x25519: XStaticSecret,
    pub mlkem_dk: MlKemDk,
}

/// The published session public key the key authority seals the CEK to.
pub struct SessionKemPublic {
    pub x25519: XPublicKey,
    pub mlkem_ek: MlKemEk,
}

/// Canonical published-pubkey encoding (`x25519(32) ‖ ML-KEM-768 ek`) — the exact
/// bytes the key authority seals to and that the decrypt boundary binds into the
/// transcript as `decrypt_session_pub`. Both sides must agree byte-for-byte.
pub fn session_public_bytes(public: &SessionKemPublic) -> Vec<u8> {
    use ml_kem::EncodedSizeUser;
    let mut v = Vec::new();
    v.extend_from_slice(public.x25519.as_bytes());
    v.extend_from_slice(public.mlkem_ek.as_bytes().as_slice());
    v
}

/// Parse a published session public key back from [`session_public_bytes`]. `None`
/// on any wrong-size/malformed encoding (fail-closed).
pub fn session_public_from_bytes(bytes: &[u8]) -> Option<SessionKemPublic> {
    use ml_kem::{Encoded, EncodedSizeUser};
    if bytes.len() < 32 {
        return None;
    }
    let (x, ek) = bytes.split_at(32);
    let xarr: [u8; 32] = x.try_into().ok()?;
    let x25519 = XPublicKey::from(xarr);
    let enc = Encoded::<MlKemEk>::try_from(ek).ok()?;
    let mlkem_ek = MlKemEk::from_bytes(&enc);
    Some(SessionKemPublic { x25519, mlkem_ek })
}

/// Reconstruct the VM session secret from its serialized parts (the x25519 static
/// secret + the ML-KEM-768 decapsulation key) — deterministic, no RNG. `None` on a
/// malformed decapsulation-key encoding (fail-closed).
pub fn session_secret_from_parts(
    x25519_secret: &[u8; 32],
    mlkem_dk_bytes: &[u8],
) -> Option<SessionKemSecret> {
    use ml_kem::{Encoded, EncodedSizeUser};
    let x25519 = XStaticSecret::from(*x25519_secret);
    let enc = Encoded::<MlKemDk>::try_from(mlkem_dk_bytes).ok()?;
    let mlkem_dk = MlKemDk::from_bytes(&enc);
    Some(SessionKemSecret { x25519, mlkem_dk })
}

/// Mint a fresh per-session hybrid KEM keypair. The decrypt boundary calls this
/// INSIDE its sandbox (the secret never leaves) and publishes
/// [`session_public_bytes`] for the key authority to seal to. Uses `OsRng` (WASI
/// `random_get` on wasm32-wasip1).
pub fn mint_session() -> (SessionKemSecret, SessionKemPublic) {
    use rand_core::OsRng;
    let mut rng = OsRng;
    let x_sk = XStaticSecret::random_from_rng(&mut rng);
    let x_pk = XPublicKey::from(&x_sk);
    let (dk, ek) = MlKem768::generate(&mut rng);
    (
        SessionKemSecret {
            x25519: x_sk,
            mlkem_dk: dk,
        },
        SessionKemPublic {
            x25519: x_pk,
            mlkem_ek: ek,
        },
    )
}

/// Verifier behind which the signature scheme is swapped (ml-dsa-65 / hybrid).
pub trait CekSealVerifier {
    fn verify(&self, msg: &[u8], sig: &[u8]) -> bool;
}

/// Real ML-DSA-65 (FIPS 204) seal-signature verifier — the shipped PQ-rail
/// signature primitive. The decrypt boundary only ever *verifies* (the key
/// authority signs the CEK-seal), so construction + verify need NO RNG and pull no
/// `getrandom`. Fail-closed: a wrong-size key encoding yields no verifier and a
/// malformed/non-matching signature verifies `false` — no panic, no state probe.
pub struct MlDsa65Verifier {
    vk: ml_dsa::VerifyingKey<ml_dsa::MlDsa65>,
}

impl MlDsa65Verifier {
    /// Build from the FIPS 204 verifying-key encoding the key authority publishes.
    /// `None` on a wrong-size/malformed encoding.
    pub fn from_encoded(bytes: &[u8]) -> Option<Self> {
        use ml_dsa::KeyInit;
        ml_dsa::VerifyingKey::<ml_dsa::MlDsa65>::new_from_slice(bytes)
            .ok()
            .map(|vk| Self { vk })
    }
}

impl CekSealVerifier for MlDsa65Verifier {
    fn verify(&self, msg: &[u8], sig: &[u8]) -> bool {
        use ml_dsa::{MlDsa65, Signature, Verifier};
        match Signature::<MlDsa65>::try_from(sig) {
            Ok(s) => self.vk.verify(msg, &s).is_ok(),
            Err(_) => false,
        }
    }
}

/// Hybrid (classical + post-quantum) seal-signature verifier — migration-period
/// profile where a classical ECDSA-P256 signature AND a PQ ML-DSA-65 signature over
/// the same payload must BOTH verify. Wire layout: `u32 ecdsa_len ‖ ecdsa(DER) ‖
/// u32 mldsa_len ‖ mldsa`. Verify-only + RNG-free.
#[cfg(feature = "hybrid")]
pub mod hybrid {
    use super::{CekSealVerifier, MlDsa65Verifier};
    use p256::ecdsa::{signature::Verifier as _, Signature as EcdsaSig, VerifyingKey as EcdsaVk};

    pub struct HybridVerifier {
        ecdsa: EcdsaVk,
        mldsa: MlDsa65Verifier,
    }

    impl HybridVerifier {
        pub fn from_encoded(ecdsa_sec1: &[u8], mldsa_vk: &[u8]) -> Option<Self> {
            let ecdsa = EcdsaVk::from_sec1_bytes(ecdsa_sec1).ok()?;
            let mldsa = MlDsa65Verifier::from_encoded(mldsa_vk)?;
            Some(Self { ecdsa, mldsa })
        }

        fn split(sig: &[u8]) -> Option<(&[u8], &[u8])> {
            fn rd<'a>(b: &'a [u8], off: &mut usize, n: usize) -> Option<&'a [u8]> {
                let end = off.checked_add(n)?;
                if end > b.len() {
                    return None;
                }
                let s = &b[*off..end];
                *off = end;
                Some(s)
            }
            let mut off = 0usize;
            let l0 = u32::from_be_bytes(rd(sig, &mut off, 4)?.try_into().ok()?) as usize;
            let ecdsa = rd(sig, &mut off, l0)?;
            let l1 = u32::from_be_bytes(rd(sig, &mut off, 4)?.try_into().ok()?) as usize;
            let mldsa = rd(sig, &mut off, l1)?;
            if off != sig.len() {
                return None;
            }
            Some((ecdsa, mldsa))
        }

        pub fn encode_signature(ecdsa_der: &[u8], mldsa: &[u8]) -> Vec<u8> {
            let mut v = Vec::with_capacity(8 + ecdsa_der.len() + mldsa.len());
            v.extend_from_slice(&(ecdsa_der.len() as u32).to_be_bytes());
            v.extend_from_slice(ecdsa_der);
            v.extend_from_slice(&(mldsa.len() as u32).to_be_bytes());
            v.extend_from_slice(mldsa);
            v
        }
    }

    impl CekSealVerifier for HybridVerifier {
        fn verify(&self, msg: &[u8], sig: &[u8]) -> bool {
            let (ecdsa_sig, mldsa_sig) = match Self::split(sig) {
                Some(halves) => halves,
                None => return false,
            };
            let es = match EcdsaSig::from_der(ecdsa_sig) {
                Ok(s) => s,
                Err(_) => return false,
            };
            self.ecdsa.verify(msg, &es).is_ok() && self.mldsa.verify(msg, mldsa_sig)
        }
    }
}

/// A PQ-hybrid sealed CEK envelope. Carries only public/sealed material; the CEK
/// exists only after a correct in-VM unwrap.
pub struct PqSealedEnvelope {
    /// Ephemeral x25519 public key (sender side of the DH half).
    pub eph_x25519_pub: [u8; 32],
    /// ML-KEM-768 encapsulation ciphertext (the PQ half).
    pub kem_ct: Ciphertext<MlKem768>,
    /// AES-256-GCM nonce.
    pub nonce: [u8; 12],
    /// AEAD-wrapped CEK (ciphertext ‖ tag).
    pub wrapped_cek: Vec<u8>,
    /// Signature over `signed_payload() ‖ aad` (scheme behind [`CekSealVerifier`]).
    pub signature: Vec<u8>,
}

impl PqSealedEnvelope {
    /// Flatten to a length-prefixed wire blob — used for transport and for the
    /// containment check that the raw CEK never appears in the sealed bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&self.eph_x25519_pub);
        let ct = self.kem_ct.as_slice();
        v.extend_from_slice(&(ct.len() as u32).to_be_bytes());
        v.extend_from_slice(ct);
        v.extend_from_slice(&self.nonce);
        v.extend_from_slice(&(self.wrapped_cek.len() as u32).to_be_bytes());
        v.extend_from_slice(&self.wrapped_cek);
        v.extend_from_slice(&(self.signature.len() as u32).to_be_bytes());
        v.extend_from_slice(&self.signature);
        v
    }

    /// Decode a sealed envelope from its [`to_bytes`](Self::to_bytes) wire form.
    /// Coarse `UnsealFailed` on any malformed/truncated input so a forged carrier
    /// cannot probe which field failed.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PqEnvelopeError> {
        fn read<'a>(b: &'a [u8], off: &mut usize, n: usize) -> Result<&'a [u8], PqEnvelopeError> {
            let end = off.checked_add(n).ok_or(PqEnvelopeError::UnsealFailed)?;
            if end > b.len() {
                return Err(PqEnvelopeError::UnsealFailed);
            }
            let s = &b[*off..end];
            *off = end;
            Ok(s)
        }
        fn read_u32(b: &[u8], off: &mut usize) -> Result<usize, PqEnvelopeError> {
            let s = read(b, off, 4)?;
            Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as usize)
        }

        let mut off = 0usize;
        let eph: [u8; 32] = read(bytes, &mut off, 32)?
            .try_into()
            .map_err(|_| PqEnvelopeError::UnsealFailed)?;
        let ct_len = read_u32(bytes, &mut off)?;
        let ct_bytes = read(bytes, &mut off, ct_len)?;
        let kem_ct =
            Ciphertext::<MlKem768>::try_from(ct_bytes).map_err(|_| PqEnvelopeError::UnsealFailed)?;
        let nonce: [u8; 12] = read(bytes, &mut off, 12)?
            .try_into()
            .map_err(|_| PqEnvelopeError::UnsealFailed)?;
        let wrapped_len = read_u32(bytes, &mut off)?;
        let wrapped_cek = read(bytes, &mut off, wrapped_len)?.to_vec();
        let sig_len = read_u32(bytes, &mut off)?;
        let signature = read(bytes, &mut off, sig_len)?.to_vec();

        Ok(PqSealedEnvelope {
            eph_x25519_pub: eph,
            kem_ct,
            nonce,
            wrapped_cek,
            signature,
        })
    }

    /// The bytes the signature covers (everything except the signature itself).
    /// Public so the decrypt boundary (which re-exports this type) can pin the
    /// signed transcript directly in its hardening tests.
    pub fn signed_payload(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&self.eph_x25519_pub);
        v.extend_from_slice(self.kem_ct.as_slice());
        v.extend_from_slice(&self.nonce);
        v.extend_from_slice(&self.wrapped_cek);
        v
    }
}

/// Derive the 32-byte AEAD wrap key from BOTH KEM shared secrets. Length-prefixed +
/// labelled so the two halves cannot be confused or truncated.
fn derive_wrap_key(x_ss: &[u8], pq_ss: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut h = Sha256::new();
    h.update(KDF_LABEL);
    h.update((x_ss.len() as u32).to_be_bytes());
    h.update(x_ss);
    h.update((pq_ss.len() as u32).to_be_bytes());
    h.update(pq_ss);
    let digest = h.finalize();
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&digest);
    key
}

/// Unwrap the CEK INSIDE the decrypt boundary (unbound). See [`hybrid_unwrap_bound`].
pub fn hybrid_unwrap(
    session: &SessionKemSecret,
    envelope: &PqSealedEnvelope,
    verifier: &impl CekSealVerifier,
) -> Result<Zeroizing<Vec<u8>>, PqEnvelopeError> {
    hybrid_unwrap_bound(session, envelope, b"", verifier)
}

/// Transcript-bound unwrap: verify the signature over `signed_payload() ‖ aad`, run
/// the hybrid KEM (x25519 DH + ML-KEM-768 decapsulate), derive the AEAD wrap key,
/// and AES-256-GCM-open the wrapped CEK with `aad` as additional authenticated data.
/// Returns the CEK in `Zeroizing`. No RNG, no outbound authority — a pure in-VM
/// transform. ANY transcript mismatch fails closed at the GCM tag or the signature,
/// before any plaintext exists.
#[allow(deprecated)] // aes-gcm 0.10 GenericArray::from_slice — kept byte-identical to the proven island
pub fn hybrid_unwrap_bound(
    session: &SessionKemSecret,
    envelope: &PqSealedEnvelope,
    aad: &[u8],
    verifier: &impl CekSealVerifier,
) -> Result<Zeroizing<Vec<u8>>, PqEnvelopeError> {
    let mut signed = envelope.signed_payload();
    signed.extend_from_slice(aad);
    if !verifier.verify(&signed, &envelope.signature) {
        return Err(PqEnvelopeError::BadSignature);
    }

    let eph_pub = XPublicKey::from(envelope.eph_x25519_pub);
    let x_ss = session.x25519.diffie_hellman(&eph_pub);

    let pq_ss = session
        .mlkem_dk
        .decapsulate(&envelope.kem_ct)
        .map_err(|_| PqEnvelopeError::DecapFailed)?;

    let wrap_key = derive_wrap_key(x_ss.as_bytes(), pq_ss.as_slice());
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&wrap_key[..]));
    let cek = cipher
        .decrypt(
            Nonce::from_slice(&envelope.nonce),
            Payload {
                msg: envelope.wrapped_cek.as_ref(),
                aad,
            },
        )
        .map_err(|_| PqEnvelopeError::UnsealFailed)?;
    Ok(Zeroizing::new(cek))
}

/// Key-authority-side seal — the counterpart of the in-VM unwrap. Unlike the
/// extracted decrypt island (where seal was test-only), this is PRODUCTION code: the
/// key authority needs to seal a recovered CEK to a decrypt session's published key.
pub mod seal {
    use super::*;
    use ml_kem::kem::Encapsulate;
    use rand_core::{OsRng, RngCore};
    use x25519_dalek::EphemeralSecret;

    /// Signer behind the same abstraction as [`CekSealVerifier`].
    pub trait CekSealSigner {
        fn sign(&self, msg: &[u8]) -> Vec<u8>;
    }

    /// Real ML-DSA-65 key-authority signer. Deterministic from seed — no RNG.
    pub struct MlDsaSealSigner {
        sk: ml_dsa::SigningKey<ml_dsa::MlDsa65>,
    }

    impl CekSealSigner for MlDsaSealSigner {
        fn sign(&self, msg: &[u8]) -> Vec<u8> {
            use ml_dsa::{SignatureEncoding, Signer};
            self.sk.sign(msg).to_bytes().to_vec()
        }
    }

    /// `(signer, verifying_key_encoding)` — the vk is what the decrypt boundary is
    /// configured to trust. Deterministic from a 32-byte seed.
    pub fn mldsa_seal_keypair(seed: [u8; 32]) -> (MlDsaSealSigner, Vec<u8>) {
        use ml_dsa::{Keypair, MlDsa65, SigningKey};
        let s: ml_dsa::B32 = seed.into();
        let sk = SigningKey::<MlDsa65>::from_seed(&s);
        let vk = sk.verifying_key().encode().to_vec();
        (MlDsaSealSigner { sk }, vk)
    }

    /// Seal a CEK to a published session public key (unbound). See [`seal_bound`].
    pub fn seal(
        public: &SessionKemPublic,
        cek: &[u8],
        signer: &impl CekSealSigner,
    ) -> PqSealedEnvelope {
        seal_bound(public, cek, b"", signer)
    }

    /// Transcript-bound seal — the key-authority counterpart of
    /// [`hybrid_unwrap_bound`]. Wraps the CEK with `aad` as AES-256-GCM additional
    /// authenticated data and signs `signed_payload() ‖ aad`, so the produced
    /// envelope only opens under the identical transcript. `aad == b""` reproduces
    /// [`seal`] exactly.
    #[allow(deprecated)] // aes-gcm 0.10 GenericArray::from_slice — kept byte-identical to the proven island
    pub fn seal_bound(
        public: &SessionKemPublic,
        cek: &[u8],
        aad: &[u8],
        signer: &impl CekSealSigner,
    ) -> PqSealedEnvelope {
        let mut rng = OsRng;
        let eph = EphemeralSecret::random_from_rng(&mut rng);
        let eph_pub = XPublicKey::from(&eph);
        let x_ss = eph.diffie_hellman(&public.x25519);
        let (kem_ct, pq_ss) = public.mlkem_ek.encapsulate(&mut rng).expect("encapsulate");

        let wrap_key = derive_wrap_key(x_ss.as_bytes(), pq_ss.as_slice());
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&wrap_key[..]));
        let mut nonce = [0u8; 12];
        rng.fill_bytes(&mut nonce);
        let wrapped_cek = cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: cek, aad })
            .expect("aead wrap");

        let mut env = PqSealedEnvelope {
            eph_x25519_pub: eph_pub.to_bytes(),
            kem_ct,
            nonce,
            wrapped_cek,
            signature: Vec::new(),
        };
        let mut signed = env.signed_payload();
        signed.extend_from_slice(aad);
        env.signature = signer.sign(&signed);
        env
    }
}

#[cfg(test)]
mod tests {
    use super::seal::*;
    use super::*;

    const SEED: [u8; 32] = [7u8; 32];

    fn cek() -> Vec<u8> {
        (0u8..16).collect()
    }

    #[test]
    fn round_trip_bound_recovers_cek_with_real_mldsa() {
        let (session_secret, session_public) = mint_session();
        let (signer, vk) = mldsa_seal_keypair(SEED);
        let verifier = MlDsa65Verifier::from_encoded(&vk).expect("verifier");
        let aad = b"transcript:principal=alice;session=s1;object=cid1";

        let env = seal_bound(&session_public, &cek(), aad, &signer);
        let recovered = hybrid_unwrap_bound(&session_secret, &env, aad, &verifier).expect("open");
        assert_eq!(recovered.as_slice(), cek().as_slice());

        // The raw CEK never appears in the sealed wire bytes.
        let wire = env.to_bytes();
        assert!(!wire.windows(cek().len()).any(|w| w == cek().as_slice()));
        // Wire round-trips through from_bytes byte-for-byte.
        let reparsed = PqSealedEnvelope::from_bytes(&wire).expect("reparse");
        assert_eq!(reparsed.to_bytes(), wire);
    }

    #[test]
    fn wrong_aad_transcript_fails_closed() {
        let (session_secret, session_public) = mint_session();
        let (signer, vk) = mldsa_seal_keypair(SEED);
        let verifier = MlDsa65Verifier::from_encoded(&vk).expect("verifier");

        let env = seal_bound(&session_public, &cek(), b"transcript-A", &signer);
        // Opening under a different transcript must fail (replay/swap defense).
        let err = hybrid_unwrap_bound(&session_secret, &env, b"transcript-B", &verifier);
        assert!(err.is_err());
    }

    #[test]
    fn wrong_session_fails_closed() {
        let (_secret_a, public_a) = mint_session();
        let (secret_b, _public_b) = mint_session();
        let (signer, vk) = mldsa_seal_keypair(SEED);
        let verifier = MlDsa65Verifier::from_encoded(&vk).expect("verifier");

        let env = seal_bound(&public_a, &cek(), b"t", &signer);
        // Sealed to session A; session B cannot open it.
        assert!(hybrid_unwrap_bound(&secret_b, &env, b"t", &verifier).is_err());
    }

    #[test]
    fn tampered_signature_fails_closed() {
        let (session_secret, session_public) = mint_session();
        let (signer, vk) = mldsa_seal_keypair(SEED);
        let verifier = MlDsa65Verifier::from_encoded(&vk).expect("verifier");

        let mut env = seal_bound(&session_public, &cek(), b"t", &signer);
        env.signature[0] ^= 0xFF;
        assert_eq!(
            hybrid_unwrap_bound(&session_secret, &env, b"t", &verifier),
            Err(PqEnvelopeError::BadSignature)
        );
    }

    #[test]
    fn wrong_verifying_key_fails_closed() {
        let (session_secret, session_public) = mint_session();
        let (signer, _vk) = mldsa_seal_keypair(SEED);
        let (_other_signer, other_vk) = mldsa_seal_keypair([9u8; 32]);
        let verifier = MlDsa65Verifier::from_encoded(&other_vk).expect("verifier");

        let env = seal_bound(&session_public, &cek(), b"t", &signer);
        assert_eq!(
            hybrid_unwrap_bound(&session_secret, &env, b"t", &verifier),
            Err(PqEnvelopeError::BadSignature)
        );
    }

    #[test]
    fn published_pubkey_round_trips() {
        let (_secret, public) = mint_session();
        let bytes = session_public_bytes(&public);
        let parsed = session_public_from_bytes(&bytes).expect("parse");
        assert_eq!(session_public_bytes(&parsed), bytes);
    }

    #[test]
    fn malformed_envelope_fails_closed_without_panic() {
        for len in [0usize, 1, 31, 32, 40, 100] {
            let blob = vec![0u8; len];
            let _ = PqSealedEnvelope::from_bytes(&blob);
        }
    }
}
