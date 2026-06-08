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
    /// An encrypted fMP4 segment produced with that CEK.
    pub encrypted_segment_b64: String,
    /// The plaintext bytes the segment's single sample must decrypt to.
    pub expected_plaintext_b64: String,
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
