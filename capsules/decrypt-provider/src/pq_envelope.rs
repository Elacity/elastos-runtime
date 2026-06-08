//! PQ-hybrid CEK-seal envelope — de-risking island for the runtime's shipped
//! decrypt rail profile (`elastos-pq-hybrid-threshold-v0`).
//!
//! This is the post-quantum analogue of the classical `envelope.rs` (P-256 ECDH
//! → AES-256-CBC, vendored from PC2). It proves the runtime PQ-hybrid profile
//! composes end to end and recovers a CEK in `Zeroizing`, so the rail can adopt
//! it the moment Anders confirms the transport (DDRM_DECRYPT_RAIL.md §PQ):
//!
//!   - **Hybrid KEM:** `x25519` DH ‖ `ML-KEM-768` (FIPS 203). The AEAD wrap key is
//!     derived from BOTH shared secrets, so confidentiality holds if EITHER
//!     primitive stays unbroken (classical OR post-quantum).
//!   - **AEAD wrap:** `AES-256-GCM` over the CEK — authenticated, so a wrong KEM
//!     secret or a tampered blob fails closed (no plaintext on error).
//!   - **Signature:** kept behind `CekSealVerifier` so the scheme is swappable —
//!     the shipped rail plugs in `ml-dsa-65` (or a hybrid `ECDSA + ml-dsa` during
//!     PC2's migration) without touching the unwrap path.
//!
//! Containment invariants (same bar as `envelope.rs`):
//!   - the CEK materializes only after a correct hybrid-KEM + AEAD open;
//!   - it is returned in `Zeroizing` and never appears in the sealed bytes;
//!   - the unwrap path needs NO RNG and NO outbound authority — it is a pure
//!     in-boundary transform, exactly like the classical path.
//!
//! NOT wired into `OpenSession`/`Render`; this is a tested island behind the
//! `pq-envelope` feature (Parallel Change). It pulls the PQ crates only when the
//! feature is enabled, leaving the default build/test surface unchanged.

#![allow(dead_code)] // rail-candidate: tested island, not yet wired into dispatch

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use ml_kem::kem::Decapsulate;
use ml_kem::{Ciphertext, KemCore, MlKem768};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey as XPublicKey, StaticSecret as XStaticSecret};
use zeroize::Zeroizing;

type MlKemDk = <MlKem768 as KemCore>::DecapsulationKey;
type MlKemEk = <MlKem768 as KemCore>::EncapsulationKey;

/// Domain separation + profile binding for the wrap-key KDF.
const KDF_LABEL: &[u8] = b"elastos-pq-hybrid-threshold-v0/cek-wrap/v1";

/// Fail-closed error surface. Messages are coarse so a forged envelope cannot
/// probe internal state (which half failed).
#[derive(Debug, PartialEq, Eq)]
pub enum PqEnvelopeError {
    BadSignature,
    DecapFailed,
    UnsealFailed,
}

/// The decrypt VM's per-session hybrid KEM secret. Mirrors PC2 `ddrm-decrypt`'s
/// per-session keypair (`session.rs`), upgraded x25519 → x25519+ML-KEM-768. The
/// secret never leaves the VM; `x25519` is zeroized on drop and the ML-KEM
/// decapsulation key holds its secret internally.
pub struct SessionKemSecret {
    pub x25519: XStaticSecret,
    pub mlkem_dk: MlKemDk,
}

/// The published session public key the key authority seals the CEK to.
pub struct SessionKemPublic {
    pub x25519: XPublicKey,
    pub mlkem_ek: MlKemEk,
}

/// Verifier behind which the signature scheme is swapped (ml-dsa-65 / hybrid).
pub trait CekSealVerifier {
    fn verify(&self, msg: &[u8], sig: &[u8]) -> bool;
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
    /// Signature over `signed_payload()` (scheme behind `CekSealVerifier`).
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

    /// The bytes the signature covers (everything except the signature itself).
    fn signed_payload(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&self.eph_x25519_pub);
        v.extend_from_slice(self.kem_ct.as_slice());
        v.extend_from_slice(&self.nonce);
        v.extend_from_slice(&self.wrapped_cek);
        v
    }
}

/// Derive the 32-byte AEAD wrap key from BOTH KEM shared secrets. Length-prefixed
/// + labelled so the two halves cannot be confused or truncated.
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

/// Unwrap the CEK INSIDE the decrypt boundary: verify the signature, run the
/// hybrid KEM (x25519 DH + ML-KEM-768 decapsulate), derive the AEAD wrap key, and
/// AEAD-open the wrapped CEK. Returns the CEK in `Zeroizing`.
///
/// This is the PQ analogue of `envelope::ecdh_unwrap`; the rail wires it in place
/// of the classical P-256 path under the PQ profile. No RNG, no outbound
/// authority — a pure in-VM transform.
pub fn hybrid_unwrap(
    session: &SessionKemSecret,
    envelope: &PqSealedEnvelope,
    verifier: &impl CekSealVerifier,
) -> Result<Zeroizing<Vec<u8>>, PqEnvelopeError> {
    if !verifier.verify(&envelope.signed_payload(), &envelope.signature) {
        return Err(PqEnvelopeError::BadSignature);
    }

    // x25519 DH half.
    let eph_pub = XPublicKey::from(envelope.eph_x25519_pub);
    let x_ss = session.x25519.diffie_hellman(&eph_pub);

    // ML-KEM-768 decapsulate half.
    let pq_ss = session
        .mlkem_dk
        .decapsulate(&envelope.kem_ct)
        .map_err(|_| PqEnvelopeError::DecapFailed)?;

    let wrap_key = derive_wrap_key(x_ss.as_bytes(), pq_ss.as_slice());
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&wrap_key[..]));
    let cek = cipher
        .decrypt(Nonce::from_slice(&envelope.nonce), envelope.wrapped_cek.as_ref())
        .map_err(|_| PqEnvelopeError::UnsealFailed)?;
    Ok(Zeroizing::new(cek))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ml_kem::kem::Encapsulate;
    use rand_core::{OsRng, RngCore};
    use x25519_dalek::EphemeralSecret;

    /// Signer behind the same abstraction as the verifier. A deterministic stub
    /// stands in for ml-dsa-65 — the round-trip proves the abstraction binds the
    /// envelope, not the specific signature primitive.
    trait CekSealSigner {
        fn sign(&self, msg: &[u8]) -> Vec<u8>;
    }
    struct StubSigner;
    impl CekSealSigner for StubSigner {
        fn sign(&self, msg: &[u8]) -> Vec<u8> {
            let mut h = Sha256::new();
            h.update(b"stub-ml-dsa-65-placeholder");
            h.update(msg);
            h.finalize().to_vec()
        }
    }
    struct StubVerifier;
    impl CekSealVerifier for StubVerifier {
        fn verify(&self, msg: &[u8], sig: &[u8]) -> bool {
            StubSigner.sign(msg).as_slice() == sig
        }
    }

    fn gen_session() -> (SessionKemSecret, SessionKemPublic) {
        let mut rng = OsRng;
        let x_sk = XStaticSecret::random_from_rng(&mut rng);
        let x_pk = XPublicKey::from(&x_sk);
        let (dk, ek) = MlKem768::generate(&mut rng);
        (
            SessionKemSecret { x25519: x_sk, mlkem_dk: dk },
            SessionKemPublic { x25519: x_pk, mlkem_ek: ek },
        )
    }

    /// Seal a CEK to a published session public key exactly as the key authority
    /// would (independently constructing the wire shape), so the round-trip pins
    /// the rail contract end to end.
    fn seal(public: &SessionKemPublic, cek: &[u8], signer: &impl CekSealSigner) -> PqSealedEnvelope {
        let mut rng = OsRng;
        // x25519 ephemeral DH half.
        let eph = EphemeralSecret::random_from_rng(&mut rng);
        let eph_pub = XPublicKey::from(&eph);
        let x_ss = eph.diffie_hellman(&public.x25519);
        // ML-KEM-768 encapsulate half.
        let (kem_ct, pq_ss) = public.mlkem_ek.encapsulate(&mut rng).expect("encapsulate");

        let wrap_key = derive_wrap_key(x_ss.as_bytes(), pq_ss.as_slice());
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&wrap_key[..]));
        let mut nonce = [0u8; 12];
        rng.fill_bytes(&mut nonce);
        let wrapped_cek = cipher
            .encrypt(Nonce::from_slice(&nonce), cek)
            .expect("aead wrap");

        let mut env = PqSealedEnvelope {
            eph_x25519_pub: eph_pub.to_bytes(),
            kem_ct,
            nonce,
            wrapped_cek,
            signature: Vec::new(),
        };
        env.signature = signer.sign(&env.signed_payload());
        env
    }

    #[test]
    fn pq_hybrid_round_trip_recovers_cek() {
        let (secret, public) = gen_session();
        let cek = [0x42u8; 16];
        let env = seal(&public, &cek, &StubSigner);

        let recovered = hybrid_unwrap(&secret, &env, &StubVerifier).expect("unwrap");
        // `recovered` is `Zeroizing<Vec<u8>>` (compile-time containment).
        assert_eq!(recovered.as_slice(), &cek, "hybrid KEM + AEAD recovers the CEK");
    }

    #[test]
    fn wrong_session_secret_fails_closed() {
        let (_secret_a, public_a) = gen_session();
        let (secret_b, _public_b) = gen_session();
        let cek = [0x42u8; 16];
        let env = seal(&public_a, &cek, &StubSigner);

        // A different session secret derives a different wrap key -> AEAD open
        // fails; no plaintext is produced.
        let err = hybrid_unwrap(&secret_b, &env, &StubVerifier).unwrap_err();
        assert_eq!(err, PqEnvelopeError::UnsealFailed);
    }

    #[test]
    fn tampered_signature_fails_closed() {
        let (secret, public) = gen_session();
        let cek = [0x42u8; 16];
        let mut env = seal(&public, &cek, &StubSigner);
        env.signature[0] ^= 0xFF;

        assert_eq!(
            hybrid_unwrap(&secret, &env, &StubVerifier).unwrap_err(),
            PqEnvelopeError::BadSignature
        );
    }

    #[test]
    fn sealed_envelope_has_no_raw_cek() {
        let (_secret, public) = gen_session();
        let cek = [0x7Eu8; 16];
        let env = seal(&public, &cek, &StubSigner);
        let bytes = env.to_bytes();
        assert!(
            !bytes.windows(cek.len()).any(|w| w == cek),
            "the raw CEK must never appear in the sealed PQ envelope"
        );
    }
}
