//! Portable golden-vector schema for the dDRM decrypt engines.
//!
//! These vectors are language-/substrate-independent fixtures (Feathers'
//! characterization-/golden-file pattern): a fixed input → expected output,
//! captured once and committed under `tests/vectors/`. They pin the engines'
//! behaviour across refactors, a rebase onto Anders' 0.4.0, and a future microVM
//! port — and the **classical** vector is byte-identical to PC2 `ddrm-decrypt`,
//! so it can be replayed against the reference implementation for cross-impl
//! confidence.
//!
//! All byte fields are base64 (STANDARD). Capturing the bytes (rather than
//! regenerating) is deliberate: the KEM/AEAD halves are randomized at seal time,
//! but every consumer path (ECDH/x25519 DH, ML-KEM decapsulate, AES open, CENC
//! decrypt) is deterministic given the captured material, so replay needs no RNG.

#![allow(dead_code)] // schema structs: not every field is read under every feature

use serde::{Deserialize, Serialize};

/// Classical CEK-seal path: P-256 ECDH envelope unwrap → CENC AES-128-CTR decrypt.
/// Mirror of PC2 `ddrm-decrypt` (envelope + cenc).
#[derive(Debug, Serialize, Deserialize)]
pub struct ClassicalVector {
    pub description: String,
    /// P-256 session secret scalar (SEC1, 32 bytes).
    pub session_secret_key_b64: String,
    /// The CEK-sealing envelope (the flat blob `envelope::parse` consumes).
    pub sealed_envelope_b64: String,
    /// The 16-byte AES-128 CEK the envelope seals (for assertion only).
    pub cek_b64: String,
    /// An encrypted fMP4 segment produced with that CEK. May be single- or
    /// multi-sample, and may use subsample (clear+encrypted) ranges.
    pub encrypted_segment_b64: String,
    /// The plaintext bytes the segment must decrypt to (the full decrypted mdat
    /// content — concatenated samples for multi-sample vectors).
    pub expected_plaintext_b64: String,
    /// Optional init segment carrying a `tenc` whose `default_per_sample_iv_size`
    /// drives the IV size. Present only for the non-default-IV-size vector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_segment_b64: Option<String>,
    /// Per-sample IV size in bytes (8 or 16). Absent ⇒ 8. Used by the PC2
    /// conformance driver to parse `senc` correctly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iv_size: Option<u8>,
}

/// Encrypt→decrypt round-trip golden: an asset sealed by `encrypt-provider`'s real
/// in-boundary engine (mint CEK+KID → CENC encrypt → mux) that `decrypt-provider`
/// must decrypt back to the original bytes. Pins the cross-invariant composition
/// (#1 produce ↔ #2 consume). The CEK is captured as the test stand-in for the
/// still-blocked transport rail (in production it arrives sealed, never in clear).
#[derive(Debug, Serialize, Deserialize)]
pub struct RoundTripVector {
    pub description: String,
    /// The in-boundary-minted Key ID (hex) the producer surfaced.
    pub kid_hex: String,
    /// The 16-byte CEK the producer minted (rail stand-in — see above).
    pub cek_b64: String,
    /// The encrypted fMP4 segment the producer emitted.
    pub encrypted_segment_b64: String,
    /// The plaintext the producer encrypted (and the consumer must recover).
    pub expected_plaintext_b64: String,
}

/// Multi-SEGMENT encrypt→decrypt round-trip golden: a real asset split into several CENC fMP4
/// media segments (DASH/fMP4 shape — many `moof+mdat` fragments) that share **one** presentation
/// CEK, with globally-unique per-sample IVs (the counter continues across segments). The consumer
/// must decrypt the WHOLE sequence segment-by-segment back to each segment's bytes. Pins the
/// multi-segment decrypt loop; the CEK is captured as the rail stand-in (as for `RoundTripVector`).
#[derive(Debug, Serialize, Deserialize)]
pub struct RoundTripMultiSegmentVector {
    pub description: String,
    /// The in-boundary-minted Key ID (hex) the producer surfaced.
    pub kid_hex: String,
    /// The single 16-byte CEK shared across every segment (rail stand-in).
    pub cek_b64: String,
    /// The encrypted fMP4 media segments, in presentation order.
    pub segments_b64: Vec<String>,
    /// The plaintext each segment must decrypt to (concatenated samples), aligned to `segments_b64`.
    pub expected_plaintexts_b64: Vec<String>,
}

/// Rail carrier wire shape (rail Option A — the decrypt VM *receives* sealed
/// material): the sealed CEK + ciphertext the runtime hands the decrypt boundary
/// on `OpenSession`, captured as a portable golden and replayed through
/// `rail_shim::decrypt_from_carrier` (not the raw engines). For the classical
/// profile the `sealed_cek` is the flat P-256 ECDH envelope, **byte-identical**
/// to the PC2-conformant `classical_cenc.json` — so this same golden is also
/// driven through PC2's public session API (`unwrap_envelope` →
/// `media::decrypt_segment`) by `scripts/pc2-conformance.sh`.
///
/// `session_secret_key_b64` is NOT part of the production carrier (the VM's
/// session secret stays in-VM); it is carried here only so the replay can
/// reconstruct the VM side of the boundary.
#[derive(Debug, Serialize, Deserialize)]
pub struct RailCarrierVector {
    pub description: String,
    /// Seal profile tag: "ClassicalP256" or "PqHybrid".
    pub profile: String,
    /// VM session secret (replay aid; never on the wire). Classical: P-256 SEC1
    /// scalar (32 bytes). PQ-hybrid: the x25519 static secret (32 bytes).
    pub session_secret_key_b64: String,
    /// PQ-hybrid only: the ML-KEM-768 decapsulation key (FIPS 203 encoded form) —
    /// the second half of the VM session secret. Absent for the classical profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mlkem_dk_b64: Option<String>,
    /// The sealed CEK as it travels in the carrier (classical: flat envelope blob;
    /// PQ-hybrid: `PqSealedEnvelope::to_bytes()`).
    pub sealed_cek_b64: String,
    /// The ciphertext fMP4 segment to decrypt.
    pub ciphertext_segment_b64: String,
    /// Optional init segment (e.g. `tenc` defaults).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_segment_b64: Option<String>,
    /// The plaintext the segment must decrypt to.
    pub expected_plaintext_b64: String,
    /// PQ-hybrid + real-signature only: the published ML-DSA-65 verifying key the
    /// `MlDsa65Verifier` is built from to verify the carrier's seal signature.
    /// Absent for stub-signed or classical carriers (the stub verifier holds no key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mldsa_vk_b64: Option<String>,
}

/// PQ-hybrid CEK-seal path: x25519+ML-KEM-768 unwrap → CENC AES-128-CTR decrypt.
/// Runtime-specific (the `elastos-pq-hybrid-threshold-v0` profile).
#[derive(Debug, Serialize, Deserialize)]
pub struct PqVector {
    pub description: String,
    /// x25519 session static secret (32 bytes).
    pub x25519_secret_b64: String,
    /// ML-KEM-768 decapsulation key (FIPS 203 encoded form).
    pub mlkem_dk_b64: String,
    /// Ephemeral x25519 public key from the seal (32 bytes).
    pub eph_x25519_pub_b64: String,
    /// ML-KEM-768 encapsulation ciphertext.
    pub kem_ct_b64: String,
    /// AES-256-GCM nonce (12 bytes).
    pub nonce_b64: String,
    /// AEAD-wrapped CEK (ciphertext ‖ tag).
    pub wrapped_cek_b64: String,
    /// Signature over the sealed payload (stub stands in for ml-dsa-65).
    pub signature_b64: String,
    /// The 16-byte CEK the envelope seals (for assertion only).
    pub cek_b64: String,
    /// An encrypted fMP4 segment produced with that CEK.
    pub encrypted_segment_b64: String,
    /// The plaintext bytes the segment's single sample must decrypt to.
    pub expected_plaintext_b64: String,
}

/// ML-DSA-65 (FIPS 204) seal-signature known-answer test: a verifying key + a
/// signature over a fixed canonical transcript. Pins the real signature primitive
/// (behind `CekSealVerifier`) across refactor/rebase/port and upstream-crate drift
/// — if `ml-dsa` changed its keygen or signature output this would stop verifying.
#[derive(Debug, Serialize, Deserialize)]
pub struct MlDsaKatVector {
    pub description: String,
    /// ML-DSA-65 verifying key (FIPS 204 `pkEncode`).
    pub verifying_key_b64: String,
    /// The canonical transcript the signature covers.
    pub transcript_b64: String,
    /// The ML-DSA-65 signature over `transcript`.
    pub signature_b64: String,
}
