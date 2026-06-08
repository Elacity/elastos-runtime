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

/// Full pre-rail dDRM data path (PREP — feature `pq-rail-prep`, not wired into
/// dispatch). Chains the PQ-hybrid CEK unwrap into the decrypt-step cenc engine,
/// i.e. the post-quantum analogue of `decrypt_sealed_segment` (Day-18 rail-prep).
///
/// `PQ-sealed envelope → hybrid_unwrap → CEK (Zeroizing) → cenc decrypt → scoped
/// output`. Mirrors PC2 `ddrm-decrypt::session::unwrap_envelope` (recover CEK) →
/// cenc decrypt, with the PQ unwrap slotting exactly where the classical
/// `ecdh_unwrap` does. The CEK lives in `Zeroizing` for its whole lifetime, is
/// consumed + zeroized by the cenc engine, and never reaches the scoped response.
///
/// This is the single operation the Hybrid rail invokes under the PQ profile once
/// Anders confirms the transport + signature scheme — proving all three
/// in-boundary engines compose end to end before the rail lands.
#[cfg(feature = "pq-rail-prep")]
pub fn decrypt_pq_sealed_segment(
    session: &SessionKemSecret,
    sealed_envelope: &PqSealedEnvelope,
    verifier: &impl CekSealVerifier,
    ciphertext_segment: &[u8],
    init_segment: Option<&[u8]>,
) -> Result<(Vec<u8>, serde_json::Value), String> {
    use base64::Engine as _;

    let cek = hybrid_unwrap(session, sealed_envelope, verifier).map_err(|err| format!("{err:?}"))?;
    // Bridge the recovered CEK into the cenc engine's command surface, held in
    // `Zeroizing` so it is scrubbed across the internal hand-off (same discipline
    // as the classical `decrypt_sealed_segment`).
    let cek_b64 = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(cek.as_slice()));
    crate::decrypt_session_segment(&cek_b64, ciphertext_segment, init_segment)
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

    // --- full pre-rail data path (feature = "pq-rail-prep") --------------------
    //
    // PQ-seal a CEK + CENC-encrypt a segment with that SAME CEK (cross-engine
    // golden), then prove the composed path (hybrid_unwrap -> cenc decrypt)
    // recovers the plaintext while the CEK never reaches the scoped response.
    // Run with: cargo test --features pq-rail-prep

    #[cfg(feature = "pq-rail-prep")]
    fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let size = (8 + content.len()) as u32;
        let mut b = size.to_be_bytes().to_vec();
        b.extend_from_slice(box_type);
        b.extend_from_slice(content);
        b
    }

    /// Minimal single-sample encrypted fMP4 segment: moof{traf{trun,senc}} +
    /// mdat{ciphertext}. Independently constructed (matches the decrypt-step
    /// golden) so the cross-engine round-trip pins the full path.
    #[cfg(feature = "pq-rail-prep")]
    fn build_encrypted_segment(plaintext: &[u8], cek: &[u8; 16], iv8: &[u8; 8]) -> Vec<u8> {
        use aes::cipher::{KeyIvInit, StreamCipher};
        type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

        let mut iv16 = [0u8; 16];
        iv16[..8].copy_from_slice(iv8);
        let mut ciphertext = plaintext.to_vec();
        let mut cipher = Aes128Ctr::new(cek.into(), (&iv16).into());
        cipher.apply_keystream(&mut ciphertext);

        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00, 0, 0, 0, 1];
        trun_content.extend_from_slice(&(plaintext.len() as u32).to_be_bytes());
        let trun = make_box(b"trun", &trun_content);

        let mut senc_content = vec![0u8, 0, 0, 0, 0, 0, 0, 1];
        senc_content.extend_from_slice(iv8);
        let senc = make_box(b"senc", &senc_content);

        let mut traf_content = trun;
        traf_content.extend_from_slice(&senc);
        let traf = make_box(b"traf", &traf_content);
        let moof = make_box(b"moof", &traf);
        let mdat = make_box(b"mdat", &ciphertext);

        let mut segment = moof;
        segment.extend_from_slice(&mdat);
        segment
    }

    #[cfg(feature = "pq-rail-prep")]
    #[test]
    fn pq_sealed_segment_decrypts_end_to_end_and_keeps_cek_off_the_boundary() {
        use base64::Engine as _;

        let (secret, public) = gen_session();
        let plaintext = b"the quick brown fox jumps over!!"; // 32 bytes
        let cek = [0x11u8; 16];
        let iv8 = [0x22u8; 8];

        // Key authority seals the CEK to the decrypt session; producer CENC-encrypts
        // the segment with that same CEK.
        let env = seal(&public, &cek, &StubSigner);
        let segment = build_encrypted_segment(plaintext, &cek, &iv8);

        // The whole pre-rail path: PQ-sealed envelope + encrypted segment ->
        // plaintext recovered inside the boundary, CEK recovered only via the
        // hybrid KEM unwrap.
        let (output, meta) =
            super::decrypt_pq_sealed_segment(&secret, &env, &StubVerifier, &segment, None).unwrap();

        let moof_len = segment.len() - (8 + plaintext.len());
        let mdat_off = moof_len + 8;
        assert_eq!(&output[mdat_off..mdat_off + plaintext.len()], plaintext);
        assert_eq!(meta["is_protected"], serde_json::json!(true));
        assert_eq!(meta["sample_count"], serde_json::json!(1));

        // Containment: the CEK never appears in the metadata that would surface,
        // and never appeared in the sealed envelope.
        let cek_b64 = base64::engine::general_purpose::STANDARD.encode(cek);
        let meta_str = serde_json::to_string(&meta).unwrap();
        assert!(!meta_str.contains(&cek_b64), "CEK must not surface in decrypt metadata");
        assert!(
            !env.to_bytes().windows(cek.len()).any(|w| w == cek),
            "CEK must not appear in the sealed PQ envelope"
        );
    }

    #[cfg(feature = "pq-rail-prep")]
    #[test]
    fn pq_sealed_segment_wrong_session_fails_closed() {
        let (_secret_a, public_a) = gen_session();
        let (secret_b, _public_b) = gen_session();
        let cek = [0x11u8; 16];
        let iv8 = [0x22u8; 8];
        let env = seal(&public_a, &cek, &StubSigner);
        let segment = build_encrypted_segment(b"the quick brown fox jumps over!!", &cek, &iv8);

        // The wrong session secret cannot unwrap the CEK -> the whole path fails
        // closed before any segment is decrypted.
        assert!(
            super::decrypt_pq_sealed_segment(&secret_b, &env, &StubVerifier, &segment, None).is_err()
        );
    }

    // --- portable golden vector (PQ-hybrid envelope + cenc) -------------------
    //
    // The PQ path is runtime-specific (PC2 has no PQ profile), so the vector pins
    // it across refactor/rebase/port. Capturing the sealed bytes also exercises
    // the ML-KEM key/ciphertext (de)serialization the live rail's wire-decode
    // needs — proving the typed envelope reconstructs from flat bytes.

    /// Regenerate the committed PQ vector. Run:
    /// `cargo test --features gen-vectors emit_pq_vector`
    #[cfg(feature = "gen-vectors")]
    #[test]
    fn emit_pq_vector() {
        use base64::Engine as _;
        use ml_kem::EncodedSizeUser;
        let b64 = base64::engine::general_purpose::STANDARD;

        let (secret, public) = gen_session();
        let cek = [0x11u8; 16];
        let iv8 = [0x22u8; 8];
        let plaintext = b"the quick brown fox jumps over!!";
        let env = seal(&public, &cek, &StubSigner);
        let segment = build_encrypted_segment(plaintext, &cek, &iv8);

        let v = crate::vector_format::PqVector {
            description: "x25519+ML-KEM-768 hybrid seal -> CENC AES-128-CTR (elastos-pq-hybrid-threshold-v0)".to_string(),
            x25519_secret_b64: b64.encode(secret.x25519.to_bytes()),
            mlkem_dk_b64: b64.encode(&secret.mlkem_dk.as_bytes()[..]),
            eph_x25519_pub_b64: b64.encode(env.eph_x25519_pub),
            kem_ct_b64: b64.encode(&env.kem_ct[..]),
            nonce_b64: b64.encode(env.nonce),
            wrapped_cek_b64: b64.encode(&env.wrapped_cek),
            signature_b64: b64.encode(&env.signature),
            cek_b64: b64.encode(cek),
            encrypted_segment_b64: b64.encode(&segment),
            expected_plaintext_b64: b64.encode(plaintext),
        };
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors");
        std::fs::create_dir_all(dir).unwrap();
        let path = format!("{dir}/pq_hybrid_cenc.json");
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        eprintln!("wrote {path}");
    }

    /// Reconstruct the typed session secret + sealed envelope from the committed
    /// flat bytes and replay the full PQ path — substrate-independent proof.
    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    fn load_pq_vector() -> (SessionKemSecret, PqSealedEnvelope, crate::vector_format::PqVector) {
        use base64::Engine as _;
        use ml_kem::{Encoded, EncodedSizeUser};
        let b64 = base64::engine::general_purpose::STANDARD;

        let v: crate::vector_format::PqVector = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/pq_hybrid_cenc.json"
        )))
        .unwrap();

        let x_bytes: [u8; 32] = b64.decode(&v.x25519_secret_b64).unwrap().try_into().unwrap();
        let x25519 = XStaticSecret::from(x_bytes);
        let dk_bytes = b64.decode(&v.mlkem_dk_b64).unwrap();
        let enc = Encoded::<MlKemDk>::try_from(dk_bytes.as_slice()).expect("ML-KEM dk size");
        let mlkem_dk = MlKemDk::from_bytes(&enc);
        let session = SessionKemSecret { x25519, mlkem_dk };

        let ct_bytes = b64.decode(&v.kem_ct_b64).unwrap();
        let kem_ct = Ciphertext::<MlKem768>::try_from(ct_bytes.as_slice()).expect("ML-KEM ct size");
        let env = PqSealedEnvelope {
            eph_x25519_pub: b64.decode(&v.eph_x25519_pub_b64).unwrap().try_into().unwrap(),
            kem_ct,
            nonce: b64.decode(&v.nonce_b64).unwrap().try_into().unwrap(),
            wrapped_cek: b64.decode(&v.wrapped_cek_b64).unwrap(),
            signature: b64.decode(&v.signature_b64).unwrap(),
        };
        (session, env, v)
    }

    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn pq_golden_vector_replays() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let (session, env, v) = load_pq_vector();

        let segment = b64.decode(&v.encrypted_segment_b64).unwrap();
        let (output, meta) =
            super::decrypt_pq_sealed_segment(&session, &env, &StubVerifier, &segment, None).unwrap();
        let expected = b64.decode(&v.expected_plaintext_b64).unwrap();
        let mdat_off = segment.len() - expected.len();
        assert_eq!(&output[mdat_off..], expected.as_slice(), "PQ vector plaintext recovered");
        assert_eq!(meta["is_protected"], serde_json::json!(true));
    }

    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn pq_golden_vector_corrupted_fails_closed() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let (session, mut env, v) = load_pq_vector();
        env.signature[0] ^= 0xFF; // tamper the signature

        let segment = b64.decode(&v.encrypted_segment_b64).unwrap();
        assert!(
            super::decrypt_pq_sealed_segment(&session, &env, &StubVerifier, &segment, None).is_err(),
            "a corrupted PQ vector must fail closed"
        );
    }
}
