//! Rail transport shim — the carrier→engine adapter for the decrypt rail
//! (`DDRM_DECRYPT_RAIL.md`, recommended **Option A**: the decrypt VM *receives*
//! VM-sealed material rather than reaching out for it).
//!
//! Everything downstream of the rail is already proven: the in-VM unwrap + cenc
//! composition exists for both the classical profile (`decrypt_sealed_segment`,
//! `rail-prep`) and the runtime PQ-hybrid profile (`decrypt_pq_sealed_segment`,
//! `pq-rail-prep`). What was missing is the thin adapter that takes the **sealed
//! carrier off the wire** and routes it to the right engine. This module is that
//! adapter, behind the `rail-shim` feature and NOT wired into `OpenSession` —
//! so the day Anders confirms the rail it is a flag flip, not a design.
//!
//! Containment is unchanged: the carrier holds only sealed/public bytes (never a
//! raw CEK); the VM's session secret stays in-VM and is never part of the carrier;
//! the CEK materializes only inside the engine, in `Zeroizing`, and never reaches
//! the scoped response.
//!
//! Mirrors PC2 `ddrm-decrypt::session::unwrap_envelope` (the VM holds the session
//! key; the envelope bytes arrive from outside) → `request_decrypt_segment`.
//!
//! What remains genuinely pending on Anders (encoded as the two profiles below):
//!   - **Q1 (dKMS-direct vs key-provider re-seal):** does not change this adapter
//!     — either way decrypt receives a sealed carrier; only *who sealed it* differs.
//!   - **Q2 (signature scheme):** the PQ profile verifies via a `CekSealVerifier`,
//!     so `ml-dsa-65` or a hybrid `ECDSA+ml-dsa` plugs in without touching this code.
//!
//! The one line `OpenSession` will add once answered:
//!   `rail_shim::decrypt_from_carrier(&vm_session_secret, &carrier, &verifier)?`
//! then map the `(bytes, meta)` into the existing scoped response.

#![allow(dead_code)] // rail-candidate: tested island, not yet wired into dispatch

use crate::pq_envelope::{CekSealVerifier, PqSealedEnvelope, SessionKemSecret};
use serde_json::Value;

/// The CEK-seal profile a carrier uses. The runtime selects one per deployment;
/// the classical profile exists only for PC2 migration parity.
pub enum SealProfile {
    /// Classical P-256 ECDH envelope (PC2 `ddrm-decrypt` parity; migration only).
    ClassicalP256,
    /// Runtime PQ-hybrid (`x25519+ml-kem-768` KEM, `ml-dsa-65` signature).
    PqHybrid,
}

/// The sealed decrypt material the runtime hands the decrypt VM on `OpenSession`
/// (rail Option A). Carries only sealed/public bytes — **never** a raw CEK.
pub struct SealedDecryptCarrier {
    pub profile: SealProfile,
    /// The CEK sealed to the VM session key: a classical envelope blob, or a
    /// `PqSealedEnvelope::to_bytes()` blob, per `profile`.
    pub sealed_cek: Vec<u8>,
    /// The ciphertext fMP4 segment to decrypt.
    pub ciphertext_segment: Vec<u8>,
    /// Optional init segment (e.g. `tenc` defaults).
    pub init_segment: Option<Vec<u8>>,
}

/// The decrypt VM's in-VM session secret. Minted in-boundary; the published
/// public key is what the key authority sealed the CEK to. Held by the VM, never
/// on the wire — which is why it is a separate argument, not a carrier field.
pub enum SessionSecret {
    ClassicalP256(p256::SecretKey),
    PqHybrid(SessionKemSecret),
}

/// Recover the CEK from the sealed carrier INSIDE the boundary and decrypt the
/// segment — the single call `OpenSession` makes once the rail is confirmed.
/// Fails closed on any profile/secret mismatch, malformed carrier, wrong session,
/// or bad signature (the underlying engines never emit plaintext on error).
pub fn decrypt_from_carrier(
    session: &SessionSecret,
    carrier: &SealedDecryptCarrier,
    verifier: &impl CekSealVerifier,
) -> Result<(Vec<u8>, Value), String> {
    decrypt_from_carrier_bound(session, carrier, b"", verifier)
}

/// Transcript-bound carrier open (Anders, Day 45 decision): `decrypt_from_carrier`
/// with the sealed CEK welded to `aad` — the canonical decrypt transcript
/// (principal/session/object/receipt/session-pubkey/suite/provider/nonce). On the
/// PQ-hybrid profile (the product target) the binding is enforced by the AEAD AAD
/// + the signature over `payload ‖ aad`, so a CEK sealed for one transcript fails
/// closed against any other. `aad == b""` is the legacy unbound behaviour.
///
/// Binding is a PQ-profile guarantee; the classical P-256 path is PC2-migration
/// compatibility only (its AES-256-CBC envelope is not AEAD), so a non-empty `aad`
/// on a classical carrier is rejected rather than silently unbound.
pub fn decrypt_from_carrier_bound(
    session: &SessionSecret,
    carrier: &SealedDecryptCarrier,
    aad: &[u8],
    verifier: &impl CekSealVerifier,
) -> Result<(Vec<u8>, Value), String> {
    let init = carrier.init_segment.as_deref();
    match (&carrier.profile, session) {
        (SealProfile::ClassicalP256, SessionSecret::ClassicalP256(sk)) => {
            if !aad.is_empty() {
                return Err("transcript binding requires the PQ-hybrid profile".to_string());
            }
            crate::decrypt_sealed_segment(sk, &carrier.sealed_cek, &carrier.ciphertext_segment, init)
        }
        (SealProfile::PqHybrid, SessionSecret::PqHybrid(secret)) => {
            let envelope =
                PqSealedEnvelope::from_bytes(&carrier.sealed_cek).map_err(|e| format!("{e:?}"))?;
            crate::pq_envelope::decrypt_pq_sealed_segment_bound(
                secret,
                &envelope,
                aad,
                verifier,
                &carrier.ciphertext_segment,
                init,
            )
        }
        _ => Err("carrier profile does not match the VM session secret".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pq_envelope::seal_support::{
        gen_session, seal, session_secret_from_parts, StubSigner, StubVerifier,
    };
    use base64::Engine as _;

    fn b64() -> base64::engine::general_purpose::GeneralPurpose {
        base64::engine::general_purpose::STANDARD
    }

    // The PQ tests need a segment encrypted with the same CEK the carrier seals.
    fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let size = (8 + content.len()) as u32;
        let mut b = size.to_be_bytes().to_vec();
        b.extend_from_slice(box_type);
        b.extend_from_slice(content);
        b
    }

    fn build_encrypted_segment(plaintext: &[u8], cek: &[u8; 16], iv8: &[u8; 8]) -> Vec<u8> {
        use aes::cipher::{KeyIvInit, StreamCipher};
        type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;
        let mut iv16 = [0u8; 16];
        iv16[..8].copy_from_slice(iv8);
        let mut ciphertext = plaintext.to_vec();
        Aes128Ctr::new(cek.into(), (&iv16).into()).apply_keystream(&mut ciphertext);

        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00, 0, 0, 0, 1];
        trun_content.extend_from_slice(&(plaintext.len() as u32).to_be_bytes());
        let trun = make_box(b"trun", &trun_content);
        let mut senc_content = vec![0u8, 0, 0, 0, 0, 0, 0, 1];
        senc_content.extend_from_slice(iv8);
        let senc = make_box(b"senc", &senc_content);
        let mut traf = trun;
        traf.extend_from_slice(&senc);
        let traf = make_box(b"traf", &traf);
        let moof = make_box(b"moof", &traf);
        let mut segment = moof;
        segment.extend_from_slice(&make_box(b"mdat", &ciphertext));
        segment
    }

    // --- classical profile (driven by the committed classical golden vector) ---

    fn classical_vector() -> Value {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/classical_cenc.json"
        )))
        .unwrap()
    }

    fn classical_carrier(v: &Value) -> SealedDecryptCarrier {
        SealedDecryptCarrier {
            profile: SealProfile::ClassicalP256,
            sealed_cek: b64().decode(v["sealed_envelope_b64"].as_str().unwrap()).unwrap(),
            ciphertext_segment: b64().decode(v["encrypted_segment_b64"].as_str().unwrap()).unwrap(),
            init_segment: None,
        }
    }

    #[test]
    fn classical_carrier_decrypts_to_plaintext() {
        let v = classical_vector();
        let sk = p256::SecretKey::from_slice(
            &b64().decode(v["session_secret_key_b64"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        let carrier = classical_carrier(&v);

        let (output, meta) =
            decrypt_from_carrier(&SessionSecret::ClassicalP256(sk), &carrier, &StubVerifier)
                .expect("carrier should decrypt");

        let expected = b64().decode(v["expected_plaintext_b64"].as_str().unwrap()).unwrap();
        let off = carrier.ciphertext_segment.len() - expected.len();
        assert_eq!(&output[off..], expected.as_slice());
        assert_eq!(meta["is_protected"], serde_json::json!(true));
    }

    #[test]
    fn classical_carrier_wrong_session_fails_closed() {
        let v = classical_vector();
        let wrong = p256::SecretKey::random(&mut rand_core::OsRng);
        let carrier = classical_carrier(&v);
        assert!(
            decrypt_from_carrier(&SessionSecret::ClassicalP256(wrong), &carrier, &StubVerifier)
                .is_err(),
            "a wrong VM session key must fail closed"
        );
    }

    #[test]
    fn classical_carrier_malformed_fails_closed() {
        let v = classical_vector();
        let sk = p256::SecretKey::from_slice(
            &b64().decode(v["session_secret_key_b64"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        let mut carrier = classical_carrier(&v);
        carrier.sealed_cek.truncate(5); // malformed envelope

        assert!(
            decrypt_from_carrier(&SessionSecret::ClassicalP256(sk), &carrier, &StubVerifier).is_err(),
            "a malformed carrier must fail closed"
        );
    }

    #[test]
    fn profile_secret_mismatch_fails_closed() {
        let v = classical_vector();
        let carrier = classical_carrier(&v); // ClassicalP256 profile
        let (secret, _public) = gen_session(); // Pq session secret
        assert!(
            decrypt_from_carrier(&SessionSecret::PqHybrid(secret), &carrier, &StubVerifier).is_err(),
            "profile/secret mismatch must fail closed"
        );
    }

    // --- PQ-hybrid profile (the shipped target) ---

    fn pq_carrier(public: &crate::pq_envelope::SessionKemPublic, cek: &[u8; 16], segment: Vec<u8>) -> SealedDecryptCarrier {
        // The key authority seals the CEK; the carrier transports its wire form.
        let env = seal(public, cek, &StubSigner);
        SealedDecryptCarrier {
            profile: SealProfile::PqHybrid,
            sealed_cek: env.to_bytes(),
            ciphertext_segment: segment,
            init_segment: None,
        }
    }

    #[test]
    fn pq_carrier_decrypts_to_plaintext() {
        let (secret, public) = gen_session();
        let plaintext = b"the quick brown fox jumps over!!";
        let cek = [0x11u8; 16];
        let iv8 = [0x22u8; 8];
        let segment = build_encrypted_segment(plaintext, &cek, &iv8);
        let carrier = pq_carrier(&public, &cek, segment.clone());

        let (output, meta) =
            decrypt_from_carrier(&SessionSecret::PqHybrid(secret), &carrier, &StubVerifier)
                .expect("pq carrier should decrypt");

        let off = segment.len() - plaintext.len();
        assert_eq!(&output[off..], plaintext);
        assert_eq!(meta["is_protected"], serde_json::json!(true));
    }

    #[test]
    fn pq_carrier_wrong_session_fails_closed() {
        let (_secret_a, public_a) = gen_session();
        let (secret_b, _public_b) = gen_session();
        let cek = [0x11u8; 16];
        let segment = build_encrypted_segment(b"the quick brown fox jumps over!!", &cek, &[0x22u8; 8]);
        let carrier = pq_carrier(&public_a, &cek, segment);

        assert!(
            decrypt_from_carrier(&SessionSecret::PqHybrid(secret_b), &carrier, &StubVerifier).is_err(),
            "a wrong PQ session secret must fail closed"
        );
    }

    #[test]
    fn pq_carrier_tampered_signature_fails_closed() {
        let (secret, public) = gen_session();
        let cek = [0x11u8; 16];
        let segment = build_encrypted_segment(b"the quick brown fox jumps over!!", &cek, &[0x22u8; 8]);
        let mut carrier = pq_carrier(&public, &cek, segment);
        // Flip the last byte (the signature tail) of the wire envelope.
        let n = carrier.sealed_cek.len();
        carrier.sealed_cek[n - 1] ^= 0xFF;

        assert!(
            decrypt_from_carrier(&SessionSecret::PqHybrid(secret), &carrier, &StubVerifier).is_err(),
            "a tampered carrier signature must fail closed"
        );
    }

    #[test]
    fn pq_carrier_malformed_fails_closed() {
        let (secret, public) = gen_session();
        let cek = [0x11u8; 16];
        let segment = build_encrypted_segment(b"the quick brown fox jumps over!!", &cek, &[0x22u8; 8]);
        let mut carrier = pq_carrier(&public, &cek, segment);
        carrier.sealed_cek.truncate(10); // malformed PQ envelope

        assert!(
            decrypt_from_carrier(&SessionSecret::PqHybrid(secret), &carrier, &StubVerifier).is_err(),
            "a malformed PQ carrier must fail closed"
        );
    }

    // --- portable carrier golden (the carrier WIRE SHAPE, replayed through the
    //     shim entrypoint and cross-checked against PC2's session API) ---

    #[cfg(not(feature = "gen-vectors"))]
    fn rail_carrier_classical() -> crate::vector_format::RailCarrierVector {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/rail_carrier_classical.json"
        )))
        .unwrap()
    }

    #[cfg(not(feature = "gen-vectors"))]
    fn classical_session(v: &crate::vector_format::RailCarrierVector) -> p256::SecretKey {
        p256::SecretKey::from_slice(&b64().decode(&v.session_secret_key_b64).unwrap()).unwrap()
    }

    /// The committed carrier golden replays through `decrypt_from_carrier` (the
    /// exact entrypoint `OpenSession` will call) — pinning the carrier wire shape,
    /// not just the engine bytes, across refactor/rebase/port.
    #[cfg(not(feature = "gen-vectors"))]
    #[test]
    fn rail_carrier_golden_replays_through_shim() {
        let v = rail_carrier_classical();
        assert_eq!(v.profile, "ClassicalP256");
        let carrier = SealedDecryptCarrier {
            profile: SealProfile::ClassicalP256,
            sealed_cek: b64().decode(&v.sealed_cek_b64).unwrap(),
            ciphertext_segment: b64().decode(&v.ciphertext_segment_b64).unwrap(),
            init_segment: v.init_segment_b64.as_ref().map(|s| b64().decode(s).unwrap()),
        };
        let (output, meta) =
            decrypt_from_carrier(&SessionSecret::ClassicalP256(classical_session(&v)), &carrier, &StubVerifier)
                .expect("carrier golden should decrypt through the shim");

        let expected = b64().decode(&v.expected_plaintext_b64).unwrap();
        let off = carrier.ciphertext_segment.len() - expected.len();
        assert_eq!(&output[off..], expected.as_slice());
        assert_eq!(meta["is_protected"], serde_json::json!(true));
    }

    /// A tampered carrier golden must fail closed through the shim too.
    #[cfg(not(feature = "gen-vectors"))]
    #[test]
    fn rail_carrier_golden_tampered_fails_closed() {
        let v = rail_carrier_classical();
        let mut sealed = b64().decode(&v.sealed_cek_b64).unwrap();
        let n = sealed.len();
        sealed[n - 1] ^= 0xFF;
        let carrier = SealedDecryptCarrier {
            profile: SealProfile::ClassicalP256,
            sealed_cek: sealed,
            ciphertext_segment: b64().decode(&v.ciphertext_segment_b64).unwrap(),
            init_segment: None,
        };
        assert!(
            decrypt_from_carrier(&SessionSecret::ClassicalP256(classical_session(&v)), &carrier, &StubVerifier)
                .is_err(),
            "a tampered carrier golden must fail closed"
        );
    }

    // --- PQ-hybrid carrier golden (runtime-only profile; no PC2 counterpart) ---

    #[cfg(not(feature = "gen-vectors"))]
    fn rail_carrier_pq() -> crate::vector_format::RailCarrierVector {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/rail_carrier_pq.json"
        )))
        .unwrap()
    }

    #[cfg(not(feature = "gen-vectors"))]
    fn pq_session(v: &crate::vector_format::RailCarrierVector) -> crate::pq_envelope::SessionKemSecret {
        let x: [u8; 32] = b64().decode(&v.session_secret_key_b64).unwrap().try_into().unwrap();
        let dk = b64()
            .decode(v.mlkem_dk_b64.as_ref().expect("PQ carrier needs mlkem_dk"))
            .unwrap();
        session_secret_from_parts(&x, &dk)
    }

    /// The PQ carrier golden replays through `decrypt_from_carrier`'s PQ branch
    /// (`from_bytes` → `decrypt_pq_sealed_segment`), pinning the runtime-only
    /// profile's carrier wire shape with no RNG at replay.
    #[cfg(not(feature = "gen-vectors"))]
    #[test]
    fn rail_carrier_pq_golden_replays_through_shim() {
        let v = rail_carrier_pq();
        assert_eq!(v.profile, "PqHybrid");
        let carrier = SealedDecryptCarrier {
            profile: SealProfile::PqHybrid,
            sealed_cek: b64().decode(&v.sealed_cek_b64).unwrap(),
            ciphertext_segment: b64().decode(&v.ciphertext_segment_b64).unwrap(),
            init_segment: v.init_segment_b64.as_ref().map(|s| b64().decode(s).unwrap()),
        };
        let (output, meta) =
            decrypt_from_carrier(&SessionSecret::PqHybrid(pq_session(&v)), &carrier, &StubVerifier)
                .expect("PQ carrier golden should decrypt through the shim");

        let expected = b64().decode(&v.expected_plaintext_b64).unwrap();
        let off = carrier.ciphertext_segment.len() - expected.len();
        assert_eq!(&output[off..], expected.as_slice());
        assert_eq!(meta["is_protected"], serde_json::json!(true));
    }

    /// A tampered PQ carrier golden must fail closed through the shim too.
    #[cfg(not(feature = "gen-vectors"))]
    #[test]
    fn rail_carrier_pq_golden_tampered_fails_closed() {
        let v = rail_carrier_pq();
        let mut sealed = b64().decode(&v.sealed_cek_b64).unwrap();
        let n = sealed.len();
        sealed[n - 1] ^= 0xFF; // corrupt the signature tail
        let carrier = SealedDecryptCarrier {
            profile: SealProfile::PqHybrid,
            sealed_cek: sealed,
            ciphertext_segment: b64().decode(&v.ciphertext_segment_b64).unwrap(),
            init_segment: None,
        };
        assert!(
            decrypt_from_carrier(&SessionSecret::PqHybrid(pq_session(&v)), &carrier, &StubVerifier)
                .is_err(),
            "a tampered PQ carrier golden must fail closed"
        );
    }

    // --- PQ carrier golden verified by the REAL ML-DSA-65 primitive -----------
    //
    // The strongest pre-rail proof: a committed carrier whose seal signature is a
    // genuine FIPS 204 ML-DSA-65 signature, replayed through the exact
    // `decrypt_from_carrier` entrypoint `OpenSession` will call, verified by the
    // production `MlDsa65Verifier` (not the stub). Pins "real PQ signature through
    // the real rail entrypoint on a portable artifact" + fail-closed.

    #[cfg(all(feature = "pq-mldsa", not(feature = "gen-vectors")))]
    fn rail_carrier_pq_mldsa() -> crate::vector_format::RailCarrierVector {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/rail_carrier_pq_mldsa.json"
        )))
        .unwrap()
    }

    #[cfg(all(feature = "pq-mldsa", not(feature = "gen-vectors")))]
    fn mldsa_verifier(v: &crate::vector_format::RailCarrierVector) -> crate::pq_envelope::mldsa::MlDsa65Verifier {
        let vk = b64()
            .decode(v.mldsa_vk_b64.as_ref().expect("real-signed PQ carrier needs mldsa_vk"))
            .unwrap();
        crate::pq_envelope::mldsa::MlDsa65Verifier::from_encoded(&vk).expect("ML-DSA-65 vk decodes")
    }

    #[cfg(all(feature = "pq-mldsa", not(feature = "gen-vectors")))]
    fn pq_mldsa_carrier(v: &crate::vector_format::RailCarrierVector) -> SealedDecryptCarrier {
        SealedDecryptCarrier {
            profile: SealProfile::PqHybrid,
            sealed_cek: b64().decode(&v.sealed_cek_b64).unwrap(),
            ciphertext_segment: b64().decode(&v.ciphertext_segment_b64).unwrap(),
            init_segment: v.init_segment_b64.as_ref().map(|s| b64().decode(s).unwrap()),
        }
    }

    /// The real-ML-DSA-65-signed carrier golden recovers plaintext through the shim
    /// when verified by the production `MlDsa65Verifier`.
    #[cfg(all(feature = "pq-mldsa", not(feature = "gen-vectors")))]
    #[test]
    fn rail_carrier_pq_mldsa_golden_replays_with_real_verifier() {
        let v = rail_carrier_pq_mldsa();
        assert_eq!(v.profile, "PqHybrid");
        let carrier = pq_mldsa_carrier(&v);
        let (output, meta) = decrypt_from_carrier(
            &SessionSecret::PqHybrid(pq_session(&v)),
            &carrier,
            &mldsa_verifier(&v),
        )
        .expect("real-ML-DSA-65 carrier golden should decrypt through the shim");

        let expected = b64().decode(&v.expected_plaintext_b64).unwrap();
        let off = carrier.ciphertext_segment.len() - expected.len();
        assert_eq!(&output[off..], expected.as_slice());
        assert_eq!(meta["is_protected"], serde_json::json!(true));
    }

    /// A tampered seal signature fails closed under the real verifier.
    #[cfg(all(feature = "pq-mldsa", not(feature = "gen-vectors")))]
    #[test]
    fn rail_carrier_pq_mldsa_golden_tampered_signature_fails_closed() {
        let v = rail_carrier_pq_mldsa();
        let mut carrier = pq_mldsa_carrier(&v);
        let n = carrier.sealed_cek.len();
        carrier.sealed_cek[n - 1] ^= 0xFF; // corrupt the signature tail
        assert!(
            decrypt_from_carrier(&SessionSecret::PqHybrid(pq_session(&v)), &carrier, &mldsa_verifier(&v))
                .is_err(),
            "a tampered ML-DSA-65 carrier signature must fail closed"
        );
    }

    /// The carrier must not verify under a DIFFERENT ML-DSA-65 verifying key.
    #[cfg(all(feature = "pq-mldsa", not(feature = "gen-vectors")))]
    #[test]
    fn rail_carrier_pq_mldsa_golden_wrong_verifying_key_fails_closed() {
        use ml_dsa::{Keypair, MlDsa65, SigningKey};
        let v = rail_carrier_pq_mldsa();
        let carrier = pq_mldsa_carrier(&v);

        // A genuine but unrelated verifying key (deterministic from a different seed).
        let other_seed: ml_dsa::B32 = [0xABu8; 32].into();
        let other_vk = SigningKey::<MlDsa65>::from_seed(&other_seed).verifying_key().encode().to_vec();
        let wrong = crate::pq_envelope::mldsa::MlDsa65Verifier::from_encoded(&other_vk).unwrap();

        assert!(
            decrypt_from_carrier(&SessionSecret::PqHybrid(pq_session(&v)), &carrier, &wrong).is_err(),
            "a carrier signed by key A must not verify under key B"
        );
    }

    /// Tampering the signed envelope body (the wrapped CEK) fails closed.
    #[cfg(all(feature = "pq-mldsa", not(feature = "gen-vectors")))]
    #[test]
    fn rail_carrier_pq_mldsa_golden_tampered_body_fails_closed() {
        let v = rail_carrier_pq_mldsa();
        let mut carrier = pq_mldsa_carrier(&v);
        let mid = carrier.sealed_cek.len() / 2; // lands in the signed envelope body
        carrier.sealed_cek[mid] ^= 0xFF;
        assert!(
            decrypt_from_carrier(&SessionSecret::PqHybrid(pq_session(&v)), &carrier, &mldsa_verifier(&v))
                .is_err(),
            "a tampered carrier body must fail closed"
        );
    }

    // --- adversarial dispatch + containment sweep (feature = "harden") --------
    //
    // `decrypt_from_carrier` is the exact entrypoint OpenSession will expose to
    // carriers minted outside this VM. These pin: (a) profile/secret mismatch both
    // directions fails closed; (b) arbitrary carrier corruption never panics and
    // never recovers plaintext; (c) no error/metadata surface leaks the CEK or the
    // plaintext.

    /// The reverse profile/secret mismatch (PQ carrier + classical secret).
    #[cfg(all(feature = "harden", not(feature = "gen-vectors")))]
    #[test]
    fn harden_pq_carrier_with_classical_secret_fails_closed() {
        let v = rail_carrier_pq();
        let carrier = SealedDecryptCarrier {
            profile: SealProfile::PqHybrid,
            sealed_cek: b64().decode(&v.sealed_cek_b64).unwrap(),
            ciphertext_segment: b64().decode(&v.ciphertext_segment_b64).unwrap(),
            init_segment: None,
        };
        let classical_secret = p256::SecretKey::random(&mut rand_core::OsRng);
        assert!(
            decrypt_from_carrier(&SessionSecret::ClassicalP256(classical_secret), &carrier, &StubVerifier)
                .is_err(),
            "a PQ carrier with a classical secret must fail closed"
        );
    }

    /// Arbitrary single-byte corruption of the sealed carrier, driven through the
    /// real verifier: never panics, never recovers plaintext.
    #[cfg(all(feature = "harden", not(feature = "gen-vectors")))]
    #[test]
    fn harden_carrier_corruption_sweep_never_decrypts() {
        let v = rail_carrier_pq_mldsa();
        let sealed = b64().decode(&v.sealed_cek_b64).unwrap();
        let segment = b64().decode(&v.ciphertext_segment_b64).unwrap();
        for i in 0..sealed.len() {
            let mut s = sealed.clone();
            s[i] ^= 0xFF;
            let carrier = SealedDecryptCarrier {
                profile: SealProfile::PqHybrid,
                sealed_cek: s,
                ciphertext_segment: segment.clone(),
                init_segment: None,
            };
            assert!(
                decrypt_from_carrier(&SessionSecret::PqHybrid(pq_session(&v)), &carrier, &mldsa_verifier(&v))
                    .is_err(),
                "carrier byte {i} flip must fail closed"
            );
        }
    }

    /// Neither the recovered metadata (happy path) nor the error string (tampered)
    /// may contain the plaintext — the carrier path leaks nothing across the
    /// boundary.
    #[cfg(all(feature = "harden", not(feature = "gen-vectors")))]
    #[test]
    fn harden_carrier_surfaces_leak_no_plaintext() {
        let v = rail_carrier_pq_mldsa();
        let expected = b64().decode(&v.expected_plaintext_b64).unwrap();
        let pt_str = String::from_utf8(expected.clone()).unwrap();

        // Happy path: scoped metadata must not contain the plaintext.
        let carrier = pq_mldsa_carrier(&v);
        let (_out, meta) =
            decrypt_from_carrier(&SessionSecret::PqHybrid(pq_session(&v)), &carrier, &mldsa_verifier(&v))
                .expect("golden should decrypt");
        let meta_str = serde_json::to_string(&meta).unwrap();
        assert!(!meta_str.contains(&pt_str), "metadata must not contain the plaintext");

        // Tampered path: the error string must not contain the plaintext either.
        let mut tampered = pq_mldsa_carrier(&v);
        let n = tampered.sealed_cek.len();
        tampered.sealed_cek[n - 1] ^= 0xFF;
        let err = decrypt_from_carrier(
            &SessionSecret::PqHybrid(pq_session(&v)),
            &tampered,
            &mldsa_verifier(&v),
        )
        .unwrap_err();
        assert!(!err.contains(&pt_str), "error string must not contain the plaintext");
    }
}
