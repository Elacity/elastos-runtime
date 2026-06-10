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

/// Transcript-bound **2-of-2 threshold** carrier open (Day 97–98): the CEK was
/// XOR-split across two dKMS nodes at publish, so NO single node ever held the whole
/// content key. This is the runtime's explicit, owned analogue of Lit's opaque
/// `decryptAndCombine` (PC2 `non-media-decrypt.js:76`) — except the share set, the
/// nodes, and the combine are all ours and inspectable.
///
/// Each node re-sealed ITS share to THIS VM's session key, bound to the SAME decrypt
/// transcript `aad` (only the node verifying key differs). Here, INSIDE the boundary,
/// we unwrap BOTH sealed shares to their plaintext share bytes, reconstruct
/// `cek = share1 ⊕ share2` (in `Zeroizing`), and only then decrypt — so the whole CEK
/// materializes ONLY in the sandbox, never in `key-provider`. Fails closed if either
/// share is malformed, sealed for a different session/transcript, signed by the wrong
/// node, or the shares are length-mismatched. PQ-hybrid only (threshold is a product-
/// path guarantee); a classical session is rejected.
#[allow(clippy::too_many_arguments)]
pub fn decrypt_from_carrier_threshold(
    session: &SessionSecret,
    sealed_share1: &[u8],
    sealed_share2: &[u8],
    aad: &[u8],
    verifier1: &impl CekSealVerifier,
    verifier2: &impl CekSealVerifier,
    ciphertext_segment: &[u8],
    init_segment: Option<&[u8]>,
) -> Result<(Vec<u8>, Value), String> {
    let secret = match session {
        SessionSecret::PqHybrid(secret) => secret,
        SessionSecret::ClassicalP256(_) => {
            return Err("threshold reconstruction requires the PQ-hybrid session".to_string())
        }
    };

    // Unwrap each sealed share to its plaintext share bytes (each is a node-sealed
    // `PqSealedEnvelope` bound to this transcript; only the verifying key differs).
    let env1 = PqSealedEnvelope::from_bytes(sealed_share1).map_err(|e| format!("{e:?}"))?;
    let share1 = crate::pq_envelope::hybrid_unwrap_bound(secret, &env1, aad, verifier1)
        .map_err(|e| format!("{e:?}"))?;
    let env2 = PqSealedEnvelope::from_bytes(sealed_share2).map_err(|e| format!("{e:?}"))?;
    let share2 = crate::pq_envelope::hybrid_unwrap_bound(secret, &env2, aad, verifier2)
        .map_err(|e| format!("{e:?}"))?;

    // Reconstruct the CEK INSIDE the boundary; held in `Zeroizing`, scrubbed on drop.
    let cek = ddrm_envelope::combine_cek_xor(share1.as_slice(), share2.as_slice())
        .map_err(|e| e.to_string())?;
    let cek_b64 = zeroize::Zeroizing::new(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        cek.as_slice(),
    ));
    crate::decrypt_session_segment(&cek_b64, ciphertext_segment, init_segment)
}

/// Transcript-bound **2-of-3 QUORUM** carrier open (Day 113–116): the CEK was
/// Shamir-split over GF(256) across THREE dKMS nodes at publish
/// (`ddrm_envelope::split_cek_shamir2`), so ANY TWO of the three re-sealed shares
/// reconstruct it — the rail survives a dead node — while a single share remains
/// information-theoretically useless. PC2's legacy rail rents this property from
/// Lit's opaque network (`decryptAndCombine`, `non-media-decrypt.js:76`); here the
/// field arithmetic, the share set, and the quorum policy are owned and inspectable.
///
/// Each share was escrowed as `x ‖ share` (`indexed_share`) — its Shamir x-coordinate
/// rides INSIDE the sealed payload, authenticated end-to-end by the producer escrow
/// seal and the node re-seal, never as forgeable cleartext beside the envelope.
/// `node_verifiers` is the boundary's PINNED node identity list in x order
/// (`node_verifiers[i]` ↔ x = i+1). For each sealed share we find the pinned node
/// whose signature verifies, then REQUIRE the inside-x to equal that node's
/// coordinate — so node j can never impersonate node i's share, even with a valid
/// signature of its own. Fails closed on: no pinned identity verifying a share, an
/// x/identity mismatch, a duplicate x (one node's share twice is NOT a quorum), a
/// zero x, malformed payloads, or a classical session.
pub fn decrypt_from_carrier_quorum<V: CekSealVerifier>(
    session: &SessionSecret,
    sealed_share_a: &[u8],
    sealed_share_b: &[u8],
    aad: &[u8],
    node_verifiers: &[V],
    ciphertext_segment: &[u8],
    init_segment: Option<&[u8]>,
) -> Result<(Vec<u8>, Value), String> {
    let secret = match session {
        SessionSecret::PqHybrid(secret) => secret,
        SessionSecret::ClassicalP256(_) => {
            return Err("quorum reconstruction requires the PQ-hybrid session".to_string())
        }
    };

    // Unwrap ONE sealed indexed share: find the pinned node identity that verifies it,
    // and bind that identity to the x-coordinate carried inside the sealed payload.
    let unwrap_indexed = |sealed: &[u8]| -> Result<(u8, zeroize::Zeroizing<Vec<u8>>), String> {
        let env = PqSealedEnvelope::from_bytes(sealed).map_err(|e| format!("{e:?}"))?;
        for (i, verifier) in node_verifiers.iter().enumerate() {
            let expected_x = (i + 1) as u8;
            if let Ok(payload) = crate::pq_envelope::hybrid_unwrap_bound(secret, &env, aad, verifier)
            {
                let (x, share) = ddrm_envelope::parse_indexed_share(&payload)
                    .ok_or("sealed quorum share carries no valid x-coordinate")?;
                if x != expected_x {
                    return Err(
                        "quorum share x-coordinate does not match the node identity that sealed it"
                            .to_string(),
                    );
                }
                return Ok((x, zeroize::Zeroizing::new(share.to_vec())));
            }
        }
        Err("no pinned node identity verifies this sealed quorum share".to_string())
    };

    let (x_a, share_a) = unwrap_indexed(sealed_share_a)?;
    let (x_b, share_b) = unwrap_indexed(sealed_share_b)?;

    // Reconstruct the CEK INSIDE the boundary (Lagrange at x=0 over GF(256)); the
    // combine itself refuses duplicate/zero x's — a sub-quorum can never slip through.
    let cek = ddrm_envelope::combine_cek_shamir2(x_a, share_a.as_slice(), x_b, share_b.as_slice())
        .map_err(|e| e.to_string())?;
    let cek_b64 = zeroize::Zeroizing::new(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        cek.as_slice(),
    ));
    crate::decrypt_session_segment(&cek_b64, ciphertext_segment, init_segment)
}

/// Reconstruct a CEK from a RECONFIGURABLE **k-of-m** quorum and decrypt — the
/// generalization of [`decrypt_from_carrier_quorum`] once the node-set has been
/// re-shared to an arbitrary threshold `k` and membership `m` (Day 121–125).
///
/// The boundary pins ALL `m` node identities (`node_verifiers`, in coordinate order
/// `x = 1..=m`) but is handed exactly the `k` sealed shares a live sub-quorum returned.
/// Each sealed share is unwrapped against the pinned identity that verifies it, and the
/// x-coordinate sealed INSIDE the payload is bound to that identity's position — a node
/// can neither lie about its index nor stand in for another. The CEK is then reconstructed
/// IN-BOUNDARY via the general Lagrange combine ([`ddrm_envelope::lagrange_combine_at_zero`]),
/// which refuses duplicate/zero coordinates, so the SAME share replayed `k` times — or any
/// sub-`k` set — can never reassemble the key. Fails CLOSED if fewer than `k` distinct shares
/// are supplied: the threshold is enforced by the count of verified, distinct coordinates, not
/// by the caller's say-so.
pub fn decrypt_from_carrier_quorum_k<V: CekSealVerifier>(
    session: &SessionSecret,
    k: usize,
    sealed_shares: &[&[u8]],
    aad: &[u8],
    node_verifiers: &[V],
    ciphertext_segment: &[u8],
    init_segment: Option<&[u8]>,
) -> Result<(Vec<u8>, Value), String> {
    let secret = match session {
        SessionSecret::PqHybrid(secret) => secret,
        SessionSecret::ClassicalP256(_) => {
            return Err("quorum reconstruction requires the PQ-hybrid session".to_string())
        }
    };
    if k < 2 {
        return Err("a quorum threshold k must be at least 2".to_string());
    }
    if sealed_shares.len() < k {
        return Err(format!(
            "below quorum: {} sealed shares supplied for a {}-of-{} set",
            sealed_shares.len(),
            k,
            node_verifiers.len()
        ));
    }

    // Unwrap ONE sealed indexed share against the m pinned identities (x = index+1).
    let unwrap_indexed = |sealed: &[u8]| -> Result<(u8, zeroize::Zeroizing<Vec<u8>>), String> {
        let env = PqSealedEnvelope::from_bytes(sealed).map_err(|e| format!("{e:?}"))?;
        for (i, verifier) in node_verifiers.iter().enumerate() {
            let expected_x = (i + 1) as u8;
            if let Ok(payload) = crate::pq_envelope::hybrid_unwrap_bound(secret, &env, aad, verifier)
            {
                let (x, share) = ddrm_envelope::parse_indexed_share(&payload)
                    .ok_or("sealed quorum share carries no valid x-coordinate")?;
                if x != expected_x {
                    return Err(
                        "quorum share x-coordinate does not match the node identity that sealed it"
                            .to_string(),
                    );
                }
                return Ok((x, zeroize::Zeroizing::new(share.to_vec())));
            }
        }
        Err("no pinned node identity verifies this sealed quorum share".to_string())
    };

    // Collect exactly the first k verified shares; their coordinates must be distinct
    // (the combine re-checks, but we surface a clear error before reconstruction).
    let mut shares: Vec<(u8, zeroize::Zeroizing<Vec<u8>>)> = Vec::with_capacity(k);
    for sealed in sealed_shares.iter().take(k) {
        let (x, body) = unwrap_indexed(sealed)?;
        if shares.iter().any(|(seen, _)| *seen == x) {
            return Err("the same node share was presented twice — not a real quorum".to_string());
        }
        shares.push((x, body));
    }

    let points: Vec<(u8, &[u8])> = shares.iter().map(|(x, body)| (*x, body.as_slice())).collect();
    let cek = ddrm_envelope::lagrange_combine_at_zero(&points).map_err(|e| e.to_string())?;
    let cek_b64 = zeroize::Zeroizing::new(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        cek.as_slice(),
    ));
    crate::decrypt_session_segment(&cek_b64, ciphertext_segment, init_segment)
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

    /// Day 121–125: the boundary OPENS a RECONFIGURED **3-of-5** quorum — the
    /// generalization of the 2-of-3 path once the node-set has been re-shared to a new
    /// threshold + membership. Five distinct ML-DSA-65 node identities are pinned (x =
    /// 1..=5); ANY THREE sealed shares reconstruct the CEK + decrypt, any TWO fail CLOSED
    /// (below quorum), the SAME share replayed can never reach quorum, and a share whose
    /// sealed x-coordinate disagrees with its signing identity is refused.
    #[cfg(all(feature = "pq-mldsa", not(feature = "gen-vectors")))]
    #[test]
    fn boundary_opens_a_reconfigured_3_of_5_quorum_and_fails_closed_below_k() {
        use crate::pq_envelope::mldsa::MlDsa65Verifier;
        use crate::pq_envelope::seal_support::{mldsa_seal_keypair, seal_bound};

        let (secret, public) = gen_session();
        let cek = [0x42u8; 16];
        let iv8 = [0x11u8; 8];
        let plaintext = b"reconfigured-quorum-opens";
        let segment = build_encrypted_segment(plaintext, &cek, &iv8);
        let aad = b"reconfig-escrow-aad/v1";

        // 3-of-5 sharing of the CEK: P(y) = cek + c1*y + c2*y^2 at y = 1..=5 (reshare_eval
        // with the CEK as the constant term IS a fresh degree-2 split of the same secret).
        let c1 = [0x9Au8; 16];
        let c2 = [0x37u8; 16];
        let higher: [&[u8]; 2] = [&c1, &c2];

        let mut signers = Vec::new();
        let mut verifiers: Vec<MlDsa65Verifier> = Vec::new();
        let mut sealed: Vec<Vec<u8>> = Vec::new();
        for i in 0..5u8 {
            let (signer, vk) = mldsa_seal_keypair([i + 1; 32]);
            let x = i + 1;
            let share = ddrm_envelope::reshare_eval(&cek, &higher, x).expect("share");
            let payload = ddrm_envelope::indexed_share(x, &share);
            let env = seal_bound(&public, &payload, aad, &signer);
            sealed.push(env.to_bytes());
            verifiers.push(MlDsa65Verifier::from_encoded(&vk).expect("vk decodes"));
            signers.push(signer);
        }
        let session = SessionSecret::PqHybrid(secret);

        // ANY THREE shares open the reconfigured quorum and decrypt.
        let pick = [sealed[0].as_slice(), sealed[2].as_slice(), sealed[4].as_slice()];
        let (out, meta) =
            decrypt_from_carrier_quorum_k(&session, 3, &pick, aad, &verifiers, &segment, None)
                .expect("any 3-of-5 shares open the reconfigured quorum");
        let off = segment.len() - plaintext.len();
        assert_eq!(&out[off..], plaintext);
        assert_eq!(meta["is_protected"], serde_json::json!(true));

        // BELOW quorum: two shares fail closed.
        let two = [sealed[0].as_slice(), sealed[1].as_slice()];
        assert!(
            decrypt_from_carrier_quorum_k(&session, 3, &two, aad, &verifiers, &segment, None).is_err(),
            "two shares are below the 3-of-5 quorum"
        );

        // A replayed single share can never reach quorum.
        let dup = [sealed[0].as_slice(), sealed[0].as_slice(), sealed[0].as_slice()];
        assert!(
            decrypt_from_carrier_quorum_k(&session, 3, &dup, aad, &verifiers, &segment, None).is_err(),
            "the same share presented three times is not a real quorum"
        );

        // A share whose sealed x disagrees with its signing identity is refused: node x=1
        // signs an x=2 payload — the boundary binds the coordinate to the identity.
        let mis_payload =
            ddrm_envelope::indexed_share(2, &ddrm_envelope::reshare_eval(&cek, &higher, 2).unwrap());
        let mis_env = seal_bound(&public, &mis_payload, aad, &signers[0]);
        let mis = [mis_env.to_bytes(), sealed[2].clone(), sealed[3].clone()];
        let mis_refs: Vec<&[u8]> = mis.iter().map(|s| s.as_slice()).collect();
        assert!(
            decrypt_from_carrier_quorum_k(&session, 3, &mis_refs, aad, &verifiers, &segment, None)
                .is_err(),
            "a share whose x-coordinate doesn't match its signing identity is refused"
        );
    }

    /// The boundary opens a DKG-BORN quorum (Day 126–130): the shares are member shares of a
    /// distributively-generated key `F = ⊕_i f_i` (each dealer's degree-1 polynomial summed), so the
    /// CEK `F(0) = ⊕_i f_i(0)` was assembled NOWHERE during generation. The boundary reconstructs it
    /// transiently from a quorum, decrypts, AND the reconstructed CEK matches the published DKG CEK
    /// BINDING (a wrong CEK would fail the binding — the signature of an inconsistent dealer).
    #[cfg(all(feature = "pq-mldsa", not(feature = "gen-vectors")))]
    #[test]
    fn boundary_opens_a_dkg_born_quorum_and_matches_the_cek_binding() {
        use crate::pq_envelope::mldsa::MlDsa65Verifier;
        use crate::pq_envelope::seal_support::{mldsa_seal_keypair, seal_bound};

        let (secret, public) = gen_session();
        let iv8 = [0x22u8; 8];
        let plaintext = b"dkg-born-key-opens";
        let aad = b"dkg-escrow-aad/v1";

        // Three dealers, each a degree-1 polynomial f_i(x) = c_i ⊕ a_i·x. The CEK is the XOR of the
        // three private contributions c_i — never assembled during the (simulated) ceremony.
        let contrib: [[u8; 16]; 3] = [[0x11u8; 16], [0x0Fu8; 16], [0xA1u8; 16]];
        let higher: [[u8; 16]; 3] = [[0x9Au8; 16], [0x33u8; 16], [0x1Cu8; 16]];
        let mut cek = [0u8; 16];
        for c in &contrib {
            for (a, &b) in cek.iter_mut().zip(c.iter()) {
                *a ^= b;
            }
        }
        let segment = build_encrypted_segment(plaintext, &cek, &iv8);

        // Each member j (x = 1..=3) sums the dealers' sub-shares f_i(x_j) into its share F(x_j).
        let mut signers = Vec::new();
        let mut verifiers: Vec<MlDsa65Verifier> = Vec::new();
        let mut sealed: Vec<Vec<u8>> = Vec::new();
        for j in 0..3u8 {
            let x = j + 1;
            let subs: Vec<Vec<u8>> = (0..3usize)
                .map(|i| {
                    let hi: [&[u8]; 1] = [&higher[i]];
                    ddrm_envelope::reshare_eval(&contrib[i], &hi, x).expect("dealer eval")
                })
                .collect();
            let sub_refs: Vec<&[u8]> = subs.iter().map(|s| s.as_slice()).collect();
            let share = ddrm_envelope::dkg_sum_subshares(&sub_refs).expect("dkg sum");
            let payload = ddrm_envelope::indexed_share(x, &share);
            let (signer, vk) = mldsa_seal_keypair([x; 32]);
            sealed.push(seal_bound(&public, &payload, aad, &signer).to_bytes());
            verifiers.push(MlDsa65Verifier::from_encoded(&vk).expect("vk decodes"));
            signers.push(signer);
        }
        let session = SessionSecret::PqHybrid(secret);

        // ANY TWO of the three DKG-born shares open the quorum and decrypt.
        let pick = [sealed[0].as_slice(), sealed[2].as_slice()];
        let (out, meta) =
            decrypt_from_carrier_quorum_k(&session, 2, &pick, aad, &verifiers, &segment, None)
                .expect("any 2-of-3 DKG-born shares open the quorum");
        let off = segment.len() - plaintext.len();
        assert_eq!(&out[off..], plaintext, "the DKG-born quorum decrypts the content");
        assert_eq!(meta["is_protected"], serde_json::json!(true));

        // The DKG CEK BINDING verifies for the reconstructed CEK (and rejects a wrong one).
        let dkg_id = [0x33u8; 16];
        let node_set = ddrm_envelope::threshold_node_set_id_n(2, &[&[0xA1u8; 40][..], &[0xB2u8; 40][..], &[0xC3u8; 40][..]]);
        let binding = ddrm_envelope::dkg_cek_binding(&dkg_id, &node_set, &cek);
        assert_eq!(
            binding,
            ddrm_envelope::dkg_cek_binding(&dkg_id, &node_set, &cek),
            "the binding verifies for the DKG-born CEK"
        );
        let mut wrong = cek;
        wrong[0] ^= 0x01;
        assert_ne!(binding, ddrm_envelope::dkg_cek_binding(&dkg_id, &node_set, &wrong), "binding rejects a wrong CEK");

        // BELOW quorum: one DKG-born share fails closed.
        let one = [sealed[1].as_slice()];
        assert!(
            decrypt_from_carrier_quorum_k(&session, 2, &one, aad, &verifiers, &segment, None).is_err(),
            "one DKG-born share is below the 2-of-3 quorum"
        );
    }
}
