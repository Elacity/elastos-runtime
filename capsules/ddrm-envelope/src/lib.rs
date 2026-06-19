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

/// Wallet-signed access-delegation layer (PC2 `SecureViewDelegation` parity).
///
/// The trustless authorization object a sovereign runtime hands the quorum: a
/// wallet-signed delegation of an ephemeral session key over a bounded window,
/// plus a per-recover request signature. Verified by each node BEFORE it
/// evaluates the on-chain access condition itself (the on-chain check lives in
/// the node, not here). Behind `access-grant` so the lean wasm decrypt boundary
/// does not pull secp256k1/JSON it never needs.
#[cfg(feature = "access-grant")]
pub mod access;

/// AV forensic-variant layer (AV Phase 5): the variant manifest schema
/// (`elastos.ddrm.av-variants/v1`) and the **canonical, RNG-free** codeword derivation that the
/// serve-time selector (Rust) and the offline forensic extractor (the Python reference under
/// `tools/av-forensics/`) must compute identically. Anchored to [`grant_watermark_digest16`] so a
/// buyer's variant sequence is bound to the same wallet-signed identity as the image marks. Behind
/// `av-variants` so the lean wasm decrypt boundary stays serde-free unless it opts in.
#[cfg(feature = "av-variants")]
pub mod av;

/// Domain separation + profile binding for the wrap-key KDF.
const KDF_LABEL: &[u8] = b"elastos-pq-hybrid-threshold-v0/cek-wrap/v1";

/// The decrypt-material suite tag this envelope implements.
pub const SUITE_PQ_HYBRID: &str = "elastos-pq-hybrid-threshold-v0";

/// Forensic-watermark anchor: the 16-byte SHA-256 prefix over a grant's EIP-191 delegation
/// signature hex. The invisible pixel-lock watermark embeds this digest (not the raw wallet) so the
/// mark is **authenticated by the buyer's own signature** — to plant it against a victim wallet an
/// attacker would need a valid grant signed by that wallet (unforgeable), and the accused cannot
/// repudiate a signature their wallet produced. Verification recomputes this from the retained
/// grant. Lives HERE (shared) so the embedder (`ddrm-media-authority`) and the verifier
/// (`decrypt-provider --extract-watermark`) compute the identical 16 bytes and can never drift
/// (Principle 12). Input is normalised (trim + lowercase hex) so re-serialisation casing can't break
/// the match. NOTE the honest bound: the delegation signature is not a hard secret, so possession of
/// the victim's grant still enables framing — this raises the bar from "anyone can plant any wallet"
/// to "only someone holding the victim's signed grant," and gives true non-repudiation; a server-key
/// MAC or opaque-token-via-custody-log is the stronger follow-up.
pub fn grant_watermark_digest16(delegation_sig_hex: &str) -> [u8; 16] {
    let normalized = delegation_sig_hex.trim().to_ascii_lowercase();
    let full = Sha256::digest(normalized.as_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&full[..16]);
    out
}

/// Canonical decrypt-transcript binding (Anders' Day-45 decision) — the single
/// encoder BOTH sides of the rail must agree on:
///   - the **key authority** computes [`DecryptTranscriptV1::to_aad`] and seals the
///     CEK to it (AES-256-GCM AAD + signed payload), via [`seal::seal_bound`];
///   - the **decrypt boundary** rebuilds the SAME transcript from the authenticated
///     request and passes its `to_aad()` to [`hybrid_unwrap_bound`].
///
/// Living here (not in either capsule) is what lets an external sealer — `key-provider`,
/// a dKMS, a Lit-compat backend — produce material the decrypt boundary opens without
/// re-implementing (and drifting from) the binding.
pub mod transcript {
    /// Domain separation + version for the transcript AAD. Bump only with a wire break.
    pub const DECRYPT_TRANSCRIPT_LABEL: &[u8] = b"elastos-ddrm/decrypt-transcript/v1";

    /// The exact field set the sealed material is welded to. Borrowed view — the
    /// caller owns the field bytes (request-authenticated on the decrypt side).
    pub struct DecryptTranscriptV1<'a> {
        pub suite_id: &'a str,
        pub provider_id: &'a str,
        pub principal_id: &'a str,
        pub session_id: &'a str,
        pub object_cid: &'a str,
        pub content_hash: &'a [u8],
        pub action: &'a str,
        pub viewer_interface: &'a str,
        pub output_kind: &'a str,
        pub expires_at: u64,
        pub release_receipt_hash: [u8; 32],
        pub decrypt_session_pub: &'a [u8],
        pub nonce: &'a [u8],
        /// 2-of-2 THRESHOLD (Day 103–104): the node-set identity backing this release
        /// ([`crate::threshold_node_set_id`] over both nodes' vks + `t`). When present, it is
        /// welded into the AAD so the sealed material is CRYPTOGRAPHICALLY bound to the EXACT
        /// set of secret-holders — a release whose node-set was swapped fails the AEAD open at
        /// the decrypt boundary itself, not only at descriptor parse. `None` on the single-node
        /// rail, where the encoding stays byte-identical to the pre-threshold transcript (the
        /// field is appended ONLY when present, after the final length-prefixed field, so no
        /// existing AAD changes and no two distinct transcripts can collide).
        pub node_set_id: Option<&'a [u8]>,
    }

    /// Bind a release receipt into the transcript by hashing its identifying fields
    /// (Anders: "release receipt hash"). Deterministic + domain-separated, so both the
    /// key authority (which seals to a transcript carrying this hash) and the decrypt
    /// boundary (which recomputes it from the authenticated receipt) derive the SAME
    /// `release_receipt_hash` — one encoder, no drift. Field set + order match PC2's
    /// release receipt identity.
    pub fn release_receipt_hash(
        schema: &str,
        request_id: &str,
        object_cid: &str,
        principal_id: &str,
        session_id: &str,
        action: &str,
        provider: &str,
        status: &str,
        issued_at: u64,
        expires_at: u64,
    ) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"elastos-ddrm/release-receipt/v1");
        for field in [
            schema,
            request_id,
            object_cid,
            principal_id,
            session_id,
            action,
            provider,
            status,
        ] {
            h.update((field.len() as u32).to_be_bytes());
            h.update(field.as_bytes());
        }
        h.update(issued_at.to_be_bytes());
        h.update(expires_at.to_be_bytes());
        h.finalize().into()
    }

    /// Domain separation + version for the **producer→authority CEK-escrow** AAD. The
    /// producer (`encrypt-provider`) seals a freshly-minted CEK to the key authority's
    /// published recipient key with this AAD; the authority recomputes the SAME AAD to
    /// recover it. Living here (not in either capsule) is the same anti-drift discipline
    /// as the decrypt transcript — one encoder both sides bind.
    pub const ESCROW_AAD_LABEL: &[u8] = b"elastos-ddrm/cek-escrow/v1";

    /// Deterministic AAD welding an escrowed CEK to its identity + destination:
    /// `label ‖ scheme ‖ kid(bytes16) ‖ recipient_pub`. So a CEK escrowed for one
    /// `{scheme, KID}` to one authority recipient cannot be opened as another — a
    /// re-target or KID-swap changes the AAD and fails closed at the GCM tag.
    pub fn escrow_aad(scheme: &str, kid_bytes16: &[u8; 16], recipient_pub: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        let mut put = |bytes: &[u8]| {
            v.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            v.extend_from_slice(bytes);
        };
        put(ESCROW_AAD_LABEL);
        put(scheme.as_bytes());
        put(kid_bytes16);
        put(recipient_pub);
        v
    }

    impl DecryptTranscriptV1<'_> {
        /// Deterministic, unambiguous AAD: a domain label then every field
        /// length-prefixed (be32 len ‖ bytes) / fixed-width, so no two distinct
        /// transcripts can collide and no field can be slid into another.
        pub fn to_aad(&self) -> Vec<u8> {
            self.to_aad_with_segments(None)
        }

        /// As [`Self::to_aad`], but additionally binds an ordered MULTI-SEGMENT asset into the
        /// transcript: `segment_digests` is the concatenation of each segment's 32-byte content
        /// digest, in presentation order (see [`segment_digests`]). It is appended ONLY when
        /// present, AFTER `node_set_id`, so a single-segment open (digests `None`) produces a
        /// BYTE-IDENTICAL AAD to [`Self::to_aad`] — while a multi-segment open is welded to the
        /// exact ordered set of segments (a reordered, dropped, added, or substituted segment
        /// changes the digest concatenation and the seal fails to unwrap → fail closed).
        pub fn to_aad_with_segments(&self, segment_digests: Option<&[u8]>) -> Vec<u8> {
            self.to_aad_with_bindings(segment_digests, None)
        }

        /// As [`Self::to_aad_with_segments`], but additionally welds a RIGHTS-DECISION
        /// binding (the rights-provider receipt hash) into the transcript. Appended ONLY
        /// when present, AFTER the multi-segment digests, so:
        ///   - a session WITHOUT a rights binding (`None`) is BYTE-IDENTICAL to
        ///     [`Self::to_aad_with_segments`] — the committed goldens replay unchanged; and
        ///   - a gated session is cryptographically bound to the EXACT rights decision that
        ///     authorized it (a seal minted under one decision cannot be replayed under
        ///     another — the AEAD open fails closed at the decrypt boundary).
        /// Both the key-authority (sealing) and the decrypt boundary (rebuilding) call this
        /// with the same `rights_receipt_hash`, so there is one encoder and no drift.
        pub fn to_aad_with_bindings(
            &self,
            segment_digests: Option<&[u8]>,
            rights_receipt_hash: Option<&[u8]>,
        ) -> Vec<u8> {
            self.to_aad_with_all_bindings(segment_digests, rights_receipt_hash, None)
        }

        /// As [`Self::to_aad_with_bindings`], but additionally welds the AV FORENSIC variant-set
        /// commitment (chunk 4 — see [`crate::av::variant_set_commitment`]) into the transcript.
        /// Appended ONLY when present, AFTER the rights binding, so:
        ///   - an asset WITHOUT variants (`None`) is BYTE-IDENTICAL to [`Self::to_aad_with_bindings`]
        ///     — every committed golden replays unchanged; and
        ///   - a fingerprinted open is cryptographically bound to the EXACT published variant set the
        ///     serve side selected from. The served bytes are already welded via `segment_digests`;
        ///     this additionally binds the *whole* committed set, so a node cannot serve a variant
        ///     outside the set, or swap the manifest, without changing the AAD and failing the CEK
        ///     unwrap closed at the decrypt boundary. The serve side (sealing) and the decrypt
        ///     boundary (rebuilding) call this with the SAME commitment — one encoder, no drift.
        pub fn to_aad_with_all_bindings(
            &self,
            segment_digests: Option<&[u8]>,
            rights_receipt_hash: Option<&[u8]>,
            variant_set_commitment: Option<&[u8]>,
        ) -> Vec<u8> {
            let mut v = Vec::new();
            let mut put = |bytes: &[u8]| {
                v.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                v.extend_from_slice(bytes);
            };
            put(DECRYPT_TRANSCRIPT_LABEL);
            put(self.suite_id.as_bytes());
            put(self.provider_id.as_bytes());
            put(self.principal_id.as_bytes());
            put(self.session_id.as_bytes());
            put(self.object_cid.as_bytes());
            put(self.content_hash);
            put(self.action.as_bytes());
            put(self.viewer_interface.as_bytes());
            put(self.output_kind.as_bytes());
            put(&self.expires_at.to_be_bytes());
            put(&self.release_receipt_hash);
            put(self.decrypt_session_pub);
            put(self.nonce);
            // Appended ONLY when present: keeps the single-node encoding byte-identical while a
            // threshold transcript can never be confused with a single-node one (the extra
            // length-prefixed field strictly extends the AAD; every prior field is already
            // length-prefixed, so the boundary between `nonce` and this field is unambiguous).
            if let Some(node_set_id) = self.node_set_id {
                put(node_set_id);
            }
            // Multi-segment binding (same strictly-extending pattern): absent for single-segment.
            if let Some(digests) = segment_digests {
                put(digests);
            }
            // Rights-decision binding (same strictly-extending pattern): absent for an
            // ungated seal, so existing transcripts are byte-identical.
            if let Some(rights) = rights_receipt_hash {
                put(rights);
            }
            // AV variant-set binding (same strictly-extending pattern): absent for a
            // non-fingerprinted open, so existing transcripts are byte-identical.
            if let Some(variant_set) = variant_set_commitment {
                put(variant_set);
            }
            v
        }
    }
}

/// The ordered MULTI-SEGMENT binding input for
/// [`transcript::DecryptTranscriptV1::to_aad_with_segments`]: the concatenation of each segment's
/// 32-byte SHA-256 content digest, in presentation order. This is the same digest that underlies
/// each segment's raw CIDv1, so binding it into the transcript welds the open to the EXACT ordered
/// set of content-addressed segments. The producer (sealing the CEK) and the decrypt boundary
/// (unwrapping it) compute this identically from the same ordered bytes; any reorder/drop/add/
/// substitute changes the result and the AEAD unwrap fails closed.
pub fn segment_digests(segments: &[&[u8]]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut out = Vec::with_capacity(segments.len() * 32);
    for seg in segments {
        out.extend_from_slice(&Sha256::digest(seg));
    }
    out
}

/// Threshold CEK share-split (2-of-2 XOR secret-sharing) — the PRODUCER side.
///
/// Splits a content key into two shares such that `cek == share1 ⊕ share2` and
/// neither share alone reveals anything about the CEK (information-theoretically
/// perfect for a uniform mask). The producer escrows EACH share to a DIFFERENT
/// dKMS node's recipient, so no single node ever holds the whole content key — the
/// runtime's explicit, owned analogue of Lit's opaque `decryptAndCombine` threshold
/// (PC2 `non-media-decrypt.js:76`), where PC2 cannot inspect the share set.
///
/// `mask` is the fresh random share (`share1`); it MUST be uniformly random and the
/// SAME length as the CEK, drawn from a CSPRNG by the caller. The RNG is kept OUT of
/// this crate so the split stays deterministic + replayable in tests and the entropy
/// lives at the boundary that owns it (the producer). Returns `(share1, share2)`.
pub fn split_cek_xor(cek: &[u8], mask: &[u8]) -> Result<(Vec<u8>, Vec<u8>), &'static str> {
    if cek.is_empty() {
        return Err("cek must be non-empty");
    }
    if cek.len() != mask.len() {
        return Err("mask length must equal cek length");
    }
    let share1 = mask.to_vec();
    let share2: Vec<u8> = cek.iter().zip(mask.iter()).map(|(c, m)| c ^ m).collect();
    Ok((share1, share2))
}

/// Reconstruct the CEK from its two XOR shares — the DECRYPT-BOUNDARY side.
///
/// `cek == share1 ⊕ share2`. Fails closed on a length mismatch so a wrong/forged
/// share of the wrong size can never silently yield a truncated key. The combine
/// MUST run ONLY inside the decrypt sandbox — never in `key-provider` — so the whole
/// CEK materializes only where the plaintext is produced. The result rides in
/// `Zeroizing` so the reconstructed key is scrubbed when the caller drops it.
pub fn combine_cek_xor(
    share1: &[u8],
    share2: &[u8],
) -> Result<zeroize::Zeroizing<Vec<u8>>, &'static str> {
    if share1.is_empty() {
        return Err("shares must be non-empty");
    }
    if share1.len() != share2.len() {
        return Err("share length mismatch");
    }
    Ok(zeroize::Zeroizing::new(
        share1.iter().zip(share2.iter()).map(|(a, b)| a ^ b).collect(),
    ))
}

// ── Shamir t-of-n CEK secret-sharing over GF(256) (Day 113–116) ────────────────
//
// The REAL threshold: `t` shares of `n` reconstruct the CEK; fewer than `t` reveal
// NOTHING (information-theoretic, like the XOR split — every byte of a sub-quorum
// view is uniformly random). This is the runtime's explicit, owned analogue of the
// t-of-n threshold living inside Lit's network for PC2's legacy `decryptAndCombine`
// rail (`non-media-decrypt.js:76`) — where t, n, and the combine are all OPAQUE to
// PC2. Ours are in this file, under golden vectors.
//
// Field: GF(2^8) with the AES reduction polynomial x^8+x^4+x^3+x+1 (0x11B). Addition
// is XOR — which is exactly why the Day 109–112 share-wise rotation primitive
// (`share' = share ⊕ delta`) carries over UNCHANGED: a proactive refresh adds a
// random polynomial q with q(0)=0, delivered to node i as `delta_i = q(x_i)`.

/// GF(2^8) multiply (AES polynomial 0x11B). BRANCHLESS + table-free (pre-audit #4): the two
/// data-dependent branches (`if b&1` add, `if high-bit` reduce) are replaced with arithmetic masks
/// so the control flow no longer depends on the secret operands — a CEK share's bits cannot be
/// recovered from a multiply-timing side channel. `0u8.wrapping_sub(bit)` is `0xFF` when `bit==1`
/// and `0x00` when `bit==0`, the canonical constant-time select (`subtle`'s `Choice` masks the same
/// way under the hood; the explicit mask keeps this dependency-free and auditable). Iteration count
/// is already fixed at 8, so the whole routine is constant-time in `a`/`b`.
fn gf256_mul(mut a: u8, mut b: u8) -> u8 {
    let mut acc = 0u8;
    for _ in 0..8 {
        // Add `a` into the accumulator iff the current low bit of `b` is set — masked, not branched.
        let add_mask = 0u8.wrapping_sub(b & 1);
        acc ^= a & add_mask;
        // Reduce by 0x1B iff `a`'s high bit is set (captured BEFORE the shift, as in the textbook form).
        let reduce_mask = 0u8.wrapping_sub((a >> 7) & 1);
        a <<= 1;
        a ^= 0x1B & reduce_mask;
        b >>= 1;
    }
    acc
}

/// GF(2^8) multiplicative inverse via a^254 (Fermat). `a` must be non-zero —
/// callers enforce distinct, non-zero share x-coordinates before division.
fn gf256_inv(a: u8) -> u8 {
    // a^254 = a^(2+4+8+16+32+64+128) by square-and-multiply.
    let mut result = 1u8;
    let mut power = a; // a^1
    let mut exp = 254u8;
    while exp > 0 {
        if exp & 1 != 0 {
            result = gf256_mul(result, power);
        }
        power = gf256_mul(power, power);
        exp >>= 1;
    }
    result
}

/// Shamir 2-of-3 CEK split — the PRODUCER side. Evaluates the degree-1 polynomial
/// `p(x) = cek[j] ⊕ coeff[j]·x` per byte at x = 1, 2, 3, yielding three shares such
/// that ANY TWO reconstruct the CEK ([`combine_cek_shamir2`]) and any SINGLE share
/// is information-theoretically useless (for a uniform `coeff`, each share byte is
/// uniform and independent of the CEK byte).
///
/// `coeff` is the fresh random degree-1 coefficient vector; it MUST be uniformly
/// random and the SAME length as the CEK, drawn from a CSPRNG by the caller — the
/// RNG stays OUT of this crate (same policy as [`split_cek_xor`]'s `mask`) so the
/// split is deterministic + replayable under golden vectors. Returns the shares in
/// x order: `[p(1), p(2), p(3)]` — share `i` (0-based) belongs to x-coordinate
/// `i + 1`. Escrow share `i` to node `i+1` and carry the x alongside (the combine
/// needs it).
pub fn split_cek_shamir2(cek: &[u8], coeff: &[u8]) -> Result<[Vec<u8>; 3], &'static str> {
    if cek.is_empty() {
        return Err("cek must be non-empty");
    }
    if cek.len() != coeff.len() {
        return Err("coeff length must equal cek length");
    }
    let eval = |x: u8| -> Vec<u8> {
        cek.iter()
            .zip(coeff.iter())
            .map(|(&c, &k)| c ^ gf256_mul(k, x))
            .collect()
    };
    Ok([eval(1), eval(2), eval(3)])
}

/// General Shamir t-of-n CEK split — the PRODUCER side, generalizing
/// [`split_cek_shamir2`] (t=2, n=3) to ANY threshold (e.g. 5-of-12, per the SCALING
/// doc). `coeffs` are the `t-1` fresh, uniformly-random degree-coefficient vectors
/// (each the SAME length as `cek`), so the degree-`(t-1)` polynomial is
/// `p(x) = cek[j] ⊕ Σ_{d=1..t-1} coeffs[d-1][j]·x^d` per byte over GF(256). Returns the
/// `n` shares `[p(1), …, p(n)]` in x order (share `i` ↔ coordinate `i+1`); ANY `t`
/// reconstruct via [`lagrange_combine_at_zero`], and any `t-1` are information-
/// theoretically useless. The CSPRNG stays OUT of this crate (same policy as
/// [`split_cek_shamir2`]'s `coeff`) so the split is deterministic + golden-vector
/// replayable. Fails closed on an empty `cek`, `n == 0` (coordinates run `1..=n`, never
/// 0 — x=0 IS the secret), or a `coeffs` entry whose length ≠ `cek`.
pub fn split_cek_shamir(cek: &[u8], coeffs: &[&[u8]], n: u8) -> Result<Vec<Vec<u8>>, &'static str> {
    if cek.is_empty() {
        return Err("cek must be non-empty");
    }
    if n == 0 {
        return Err("n must be at least 1");
    }
    for c in coeffs {
        if c.len() != cek.len() {
            return Err("each coeff length must equal cek length");
        }
    }
    let eval = |x: u8| -> Vec<u8> {
        // powers[d] = x^(d+1), built incrementally so no general pow is needed.
        let mut powers = Vec::with_capacity(coeffs.len());
        let mut p = x;
        for _ in 0..coeffs.len() {
            powers.push(p);
            p = gf256_mul(p, x);
        }
        (0..cek.len())
            .map(|j| {
                let mut acc = cek[j];
                for (d, c) in coeffs.iter().enumerate() {
                    acc ^= gf256_mul(c[j], powers[d]);
                }
                acc
            })
            .collect()
    };
    Ok((1..=n).map(eval).collect())
}

/// Shamir t=2 CEK reconstruction from ANY TWO indexed shares — the DECRYPT-BOUNDARY
/// side. Lagrange interpolation at x=0 over GF(256), per byte:
/// `cek[j] = share_a[j]·(x_b/(x_a⊕x_b)) ⊕ share_b[j]·(x_a/(x_a⊕x_b))`.
///
/// Fails closed on: a zero x (x=0 IS the secret — accepting it would let a forged
/// "share" name the CEK's own coordinate), duplicate x's (two copies of one node's
/// share are NOT a quorum), a length mismatch, or empty shares. Like
/// [`combine_cek_xor`], this MUST run ONLY inside the decrypt sandbox, and the
/// result rides in `Zeroizing`.
pub fn combine_cek_shamir2(
    x_a: u8,
    share_a: &[u8],
    x_b: u8,
    share_b: &[u8],
) -> Result<zeroize::Zeroizing<Vec<u8>>, &'static str> {
    if share_a.is_empty() {
        return Err("shares must be non-empty");
    }
    if share_a.len() != share_b.len() {
        return Err("share length mismatch");
    }
    if x_a == 0 || x_b == 0 {
        return Err("share x-coordinate must be non-zero");
    }
    if x_a == x_b {
        return Err("a quorum needs two DISTINCT share x-coordinates");
    }
    // Lagrange basis at 0: l_a = x_b/(x_a+x_b), l_b = x_a/(x_a+x_b) (+ is XOR).
    let denom_inv = gf256_inv(x_a ^ x_b);
    let l_a = gf256_mul(x_b, denom_inv);
    let l_b = gf256_mul(x_a, denom_inv);
    Ok(zeroize::Zeroizing::new(
        share_a
            .iter()
            .zip(share_b.iter())
            .map(|(&a, &b)| gf256_mul(a, l_a) ^ gf256_mul(b, l_b))
            .collect(),
    ))
}

/// Reconstruct a 2-of-n CEK from MORE THAN the threshold of indexed shares WITH
/// CHEATER DETECTION (pre-audit finding #1). Given ≥ 3 distinct, non-zero indexed
/// shares of the SAME degree-1 secret polynomial, EVERY pair must Lagrange-combine
/// (`combine_cek_shamir2`) to the SAME CEK — because three or more colinear points
/// determine one line. A single Byzantine node returning a validly-sealed,
/// validly-indexed, but **wrong-valued** share is off that line, so the pairs that
/// include it disagree with the pair that excludes it: the inconsistency is DETECTED
/// and the open FAILS CLOSED rather than decrypting under a silently-wrong key.
///
/// With exactly 2 shares there is nothing to cross-check (any two points define a
/// line) — callers that hold only the threshold must fall back to the published
/// [`cek_commitment`] for integrity; this function REFUSES `< 3` shares so a caller
/// can never *think* it got cheater detection from a bare quorum. All candidate CEKs
/// are compared in CONSTANT TIME (`subtle`) so a near-miss reveals nothing about the
/// true key. Result rides in `Zeroizing`; MUST run only inside the decrypt sandbox.
pub fn combine_cek_shamir2_checked(
    shares: &[(u8, &[u8])],
) -> Result<zeroize::Zeroizing<Vec<u8>>, &'static str> {
    use subtle::ConstantTimeEq;
    if shares.len() < 3 {
        return Err("cheater detection needs at least three shares (above the 2-of-n threshold)");
    }
    // Reconstruct from every distinct pair; combine_cek_shamir2 enforces non-zero,
    // distinct coordinates and equal lengths, so a duplicate/zero coordinate or a
    // length mismatch fails closed here.
    let mut reference: Option<zeroize::Zeroizing<Vec<u8>>> = None;
    for i in 0..shares.len() {
        for j in (i + 1)..shares.len() {
            let (x_a, share_a) = shares[i];
            let (x_b, share_b) = shares[j];
            let candidate = combine_cek_shamir2(x_a, share_a, x_b, share_b)?;
            match &reference {
                None => reference = Some(candidate),
                Some(reference) => {
                    if reference.len() != candidate.len()
                        || !bool::from(reference.ct_eq(candidate.as_slice()))
                    {
                        return Err("quorum shares are inconsistent — a member returned a wrong-valued share (Byzantine fault); the open fails closed");
                    }
                }
            }
        }
    }
    reference.ok_or("no candidate CEK was reconstructed")
}

/// Prefix a Shamir share with its x-coordinate for escrow — `x ‖ share` — so the
/// index rides INSIDE the sealed envelope (authenticated by the escrow seal + every
/// node re-seal), never as forgeable cleartext JSON beside it. The boundary parses
/// it back with [`parse_indexed_share`] and fails closed on a zero/out-of-range x.
pub fn indexed_share(x: u8, share: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(1 + share.len());
    v.push(x);
    v.extend_from_slice(share);
    v
}

/// Parse `x ‖ share` back ([`indexed_share`]). `None` on an empty payload or a
/// zero x (never a valid Shamir coordinate — x=0 is the secret itself).
pub fn parse_indexed_share(payload: &[u8]) -> Option<(u8, &[u8])> {
    let (&x, share) = payload.split_first()?;
    if x == 0 || share.is_empty() {
        return None;
    }
    Some((x, share))
}

/// Shamir 2-of-3 PROACTIVE-REFRESH delta for ONE node (Day 117–120) — the QUORUM
/// generalization of the 2-of-2 XOR rotation delta. A proactive refresh adds a
/// degree-1 polynomial `q(x) = refresh_coeff·x` (per byte, over GF(256)) to every
/// node's share. Because `q(0) = 0`, the reconstructed secret `p(0)` is INVARIANT
/// (`p'(0) = p(0) ⊕ q(0) = cek`) while every share moves to a NEW polynomial
/// `p'(x) = p(x) ⊕ q(x)` — so an OLD captured share (on `p`) is dead next to a
/// refreshed one (on `p'`): the two no longer interpolate to the CEK.
///
/// The escrowed quorum payload is the INDEXED share `x ‖ p(x)` ([`indexed_share`]),
/// and the node's `rotate_share` blind-XORs the WHOLE payload with the delta it is
/// handed (`share' = share ⊕ delta` — the SAME byte op the 2-of-2 XOR rail uses, so
/// the node code is UNCHANGED across schemes). To keep that op correct here this
/// returns `0x00 ‖ q(x)`: the leading zero leaves the x-coordinate prefix UNTOUCHED
/// (the successor must still answer to the same coordinate the decrypt boundary
/// pins) and the body refreshes `p(x) → p(x) ⊕ refresh_coeff·x`.
///
/// The CONTRAST with XOR is the whole point of t-of-n: the 2-of-2 rail hands every
/// node the SAME delta; the quorum hands each node a DIFFERENT, coordinate-bound
/// delta derived from ONE fresh `refresh_coeff` — `q(x_i)` — so a single shared mask
/// would corrupt the polynomial. `refresh_coeff` MUST be a fresh CSPRNG draw the SAME
/// length as the share body (the RNG stays OUT of this crate, same policy as
/// [`split_cek_shamir2`]'s `coeff`). Fails closed on a zero coordinate (x=0 is the
/// secret, never a node) or an empty `refresh_coeff`.
pub fn shamir_refresh_delta(refresh_coeff: &[u8], x: u8) -> Result<Vec<u8>, &'static str> {
    if x == 0 {
        return Err("share x-coordinate must be non-zero");
    }
    if refresh_coeff.is_empty() {
        return Err("refresh_coeff must be non-empty");
    }
    let mut delta = Vec::with_capacity(1 + refresh_coeff.len());
    delta.push(0u8); // the indexed-share x prefix is preserved across the rotation
    delta.extend(refresh_coeff.iter().map(|&c| gf256_mul(c, x)));
    Ok(delta)
}

/// General Lagrange interpolation at x=0 over GF(256) — the secret-recovery /
/// share-combine workhorse generalized to ANY number of points (the t=2
/// [`combine_cek_shamir2`] is the two-point special case). Given the points
/// `(x_i, value_i)`, returns `Σ value_i · λ_i` where `λ_i = Π_{l≠i} x_l/(x_l ⊕ x_i)`
/// is the Lagrange basis evaluated at 0 (negation is identity in GF(2^8), so
/// `0 ⊖ x_l = x_l` and `x_i ⊖ x_l = x_i ⊕ x_l`).
///
/// Used in TWO places by quorum RECONFIGURATION (Day 121–125): a new node combines
/// the sub-shares an OLD quorum sent it into its new share (the points are the old
/// CONTRIBUTORS' coordinates), and the decrypt boundary reconstructs the CEK from k
/// new shares (the points are the new nodes' coordinates). Fails closed on an empty
/// set, a zero coordinate (x=0 IS the secret), duplicate coordinates (not a real
/// quorum), a length mismatch, or empty values. Result rides in `Zeroizing`.
pub fn lagrange_combine_at_zero(
    points: &[(u8, &[u8])],
) -> Result<zeroize::Zeroizing<Vec<u8>>, &'static str> {
    if points.is_empty() {
        return Err("need at least one share to combine");
    }
    let len = points[0].1.len();
    if len == 0 {
        return Err("shares must be non-empty");
    }
    for &(x, value) in points {
        if x == 0 {
            return Err("share x-coordinate must be non-zero");
        }
        if value.len() != len {
            return Err("share length mismatch");
        }
    }
    for i in 0..points.len() {
        for j in (i + 1)..points.len() {
            if points[i].0 == points[j].0 {
                return Err("a quorum needs DISTINCT share x-coordinates");
            }
        }
    }
    let mut acc = zeroize::Zeroizing::new(vec![0u8; len]);
    for (i, &(xi, value_i)) in points.iter().enumerate() {
        let mut num = 1u8;
        let mut den = 1u8;
        for (j, &(xj, _)) in points.iter().enumerate() {
            if i == j {
                continue;
            }
            num = gf256_mul(num, xj);
            den = gf256_mul(den, xi ^ xj);
        }
        let lambda = gf256_mul(num, gf256_inv(den));
        for (a, &b) in acc.iter_mut().zip(value_i.iter()) {
            *a ^= gf256_mul(lambda, b);
        }
    }
    Ok(acc)
}

/// Quorum RECONFIGURATION sub-share evaluation (Day 121–125) — the t-of-n → k-of-m
/// re-sharing primitive. An OLD quorum member holding `share` (= `p(x_i)`, its single
/// point of the current degree-(t−1) polynomial) draws a FRESH degree-(k−1) polynomial
/// whose CONSTANT TERM is its own share — `q_i(y) = share ⊕ Σ_{d=1..k-1} higher[d-1]·y^d`
/// over GF(256) — and evaluates it at a NEW node's coordinate `y` to produce the
/// sub-share `q_i(y)` it seals to that new node.
///
/// Why this reconfigures the secret WITHOUT ever reassembling it: define
/// `P(y) = Σ_i λ_i · q_i(y)` over an old quorum (the new node's combine, [`lagrange_combine_at_zero`]
/// with the old contributors' λ_i). `P` is degree (k−1), and `P(0) = Σ_i λ_i · q_i(0)
/// = Σ_i λ_i · p(x_i) = p(0) = CEK` — so the new shares lie on a FRESH degree-(k−1)
/// polynomial through the SAME secret. The new threshold is k, the new membership is m,
/// and an OLD share (on `p`) is dead against the new set (on `P`). Each member only ever
/// touches its OWN share; the CEK never exists anywhere during reconfiguration.
///
/// `higher` holds the (k−1) fresh random coefficient vectors (each the SAME length as
/// `share`, CSPRNG-drawn by the caller — the RNG stays OUT of this crate). `k` is
/// `higher.len() + 1`; `k = 1` (no higher coefficients) is a degenerate constant sharing
/// and is refused. Fails closed on a zero coordinate, empty/!-matching coefficient lengths.
pub fn reshare_eval(share: &[u8], higher: &[&[u8]], y: u8) -> Result<Vec<u8>, &'static str> {
    if share.is_empty() {
        return Err("share must be non-empty");
    }
    if y == 0 {
        return Err("new share coordinate must be non-zero");
    }
    if higher.is_empty() {
        return Err("re-sharing needs at least one higher coefficient (k must be ≥ 2)");
    }
    for c in higher {
        if c.len() != share.len() {
            return Err("re-sharing coefficient length must equal the share length");
        }
    }
    let mut out = share.to_vec();
    let mut y_pow = 1u8; // y^0
    for c in higher {
        y_pow = gf256_mul(y_pow, y); // y^d for d = 1..k-1
        for (o, &cd) in out.iter_mut().zip(c.iter()) {
            *o ^= gf256_mul(cd, y_pow);
        }
    }
    Ok(out)
}

/// DISTRIBUTED KEY GENERATION sub-share SUM (Day 126–130) — the new node's combine in a
/// Joint-Feldman DKG. The CEK is BORN distributed: every member `i` (acting as a DEALER)
/// draws a FRESH degree-(t−1) polynomial `f_i` with a RANDOM constant term `c_i = f_i(0)`
/// (its private contribution) — [`reshare_eval`] is exactly that polynomial evaluator, with
/// the constant term being a fresh contribution instead of a recovered share — and routes
/// each member `j` the sub-share `f_i(x_j)`. Member `j` SUMS the sub-shares it received into
/// its final share: `share_j = ⊕_i f_i(x_j) = F(x_j)` where `F = ⊕_i f_i` (addition in
/// GF(2^8) is XOR). `F` is degree (t−1) and `F(0) = ⊕_i f_i(0) = ⊕_i c_i = CEK`.
///
/// Why this CLOSES the "whole CEK at birth" window: no member ever holds the CEK. Each member
/// knows ONLY its own `c_i` (one addend of the sum) and ends holding ONE point of `F`. The CEK
/// `⊕_i c_i` is never assembled anywhere during generation — it materializes ONLY transiently
/// inside a decrypt (or the producer's encrypt) boundary at open time, reconstructed from a
/// quorum via [`lagrange_combine_at_zero`]. A single member's contribution `c_i` is independent
/// of the CEK (XOR of a uniform addend), and `t−1` members learn nothing of `F(0)`.
///
/// Fails closed on an empty set, empty sub-shares, or a length mismatch. Result rides in
/// `Zeroizing` (it is the node's secret share material).
pub fn dkg_sum_subshares(
    subshares: &[&[u8]],
) -> Result<zeroize::Zeroizing<Vec<u8>>, &'static str> {
    if subshares.is_empty() {
        return Err("need at least one dealer sub-share to assemble a DKG share");
    }
    let len = subshares[0].len();
    if len == 0 {
        return Err("sub-shares must be non-empty");
    }
    let mut acc = zeroize::Zeroizing::new(vec![0u8; len]);
    for s in subshares {
        if s.len() != len {
            return Err("sub-share length mismatch");
        }
        for (a, &b) in acc.iter_mut().zip(s.iter()) {
            *a ^= b;
        }
    }
    Ok(acc)
}

/// Domain label for a DKG CEK BINDING (Day 126–130). Separated from every other hash domain so a
/// binding can never be confused with a node-set id or a transcript hash.
pub const DKG_CEK_BINDING_DOMAIN: &[u8] = b"elastos.dkms.authority/dkg-cek-binding/v1";

/// A public, hiding+binding COMMITMENT to a DKG-born CEK: `SHA-256(DOMAIN ‖ lp(dkg_id) ‖
/// lp(node_set_id) ‖ lp(cek))`. Computed ONCE by the party that must learn the CEK to use it (the
/// producer, which materializes it transiently in-boundary to encrypt content) and published in the
/// descriptor. At open the boundary reconstructs the CEK from its quorum and re-derives this binding:
/// any quorum that reconstructs a DIFFERENT value (the signature of an INCONSISTENT dealer who shared
/// a malformed polynomial — different t-subsets would then disagree) fails the check, so a corrupt
/// contribution is CAUGHT at open and the disagreeing dealer is localizable. The CEK is never
/// revealed by the binding (pre-image resistance) — only a holder that already reconstructed the
/// exact CEK can reproduce it. Pure, no RNG; the single source of truth both the producer and the
/// boundary share.
pub fn dkg_cek_binding(dkg_id: &[u8], node_set_id: &[u8], cek: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    let bound = lp_concat(DKG_CEK_BINDING_DOMAIN, &[dkg_id, node_set_id, cek]);
    h.update(&bound);
    let digest = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..32]);
    out
}

/// Domain label for a quorum CEK COMMITMENT (the generalization of [`dkg_cek_binding`]
/// to ANY threshold-split CEK, DKG-born or producer-split). Separated from every other
/// hash domain so a commitment can never be confused with a node-set id, a transcript
/// hash, or a DKG binding.
pub const CEK_COMMITMENT_DOMAIN: &[u8] = b"elastos.dkms.authority/cek-commitment/v1";

/// A public, hiding+binding COMMITMENT to a threshold-split CEK:
/// `SHA-256(DOMAIN ‖ lp(node_set_id) ‖ lp(cek))`.
///
/// This is the integrity backstop for the live quorum/threshold open (pre-audit finding #1):
/// the AES-CTR content layer is unauthenticated, so a single Byzantine node returning a
/// validly-sealed, validly-indexed, but **wrong-valued** share would combine into a SILENTLY
/// WRONG CEK with no error. The producer — the only party that legitimately materializes the
/// CEK (transiently, in-boundary, to encrypt) — computes this commitment ONCE at publish and
/// publishes it alongside the escrow. At open, the decrypt boundary reconstructs the CEK from
/// its quorum and re-derives this commitment ([`verify_cek_commitment`]): a wrong-valued share
/// yields a CEK whose commitment does NOT match, so the open FAILS CLOSED instead of decrypting
/// to garbage (benign for media, catastrophic for the agent-key-custody roadmap).
///
/// Bound to `node_set_id` so a commitment published for one quorum cannot be replayed against a
/// different one; the CEK is unique per content, so the commitment is effectively content-bound.
/// The CEK is never revealed (pre-image resistance) — only a holder that already reconstructed
/// the exact CEK can reproduce it. Pure, no RNG; the single source of truth the producer and the
/// decrypt boundary share.
pub fn cek_commitment(node_set_id: &[u8], cek: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    let bound = lp_concat(CEK_COMMITMENT_DOMAIN, &[node_set_id, cek]);
    h.update(&bound);
    let digest = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..32]);
    out
}

/// Verify a reconstructed CEK against its published [`cek_commitment`], in CONSTANT TIME.
///
/// `true` iff `cek_commitment(node_set_id, cek) == expected`. The comparison is constant-time
/// (`subtle::ConstantTimeEq`) so the match does not leak — defense for the agent-key-custody
/// path where the boundary may run co-resident with an adversary. The decrypt boundary calls
/// this on the reconstructed CEK BEFORE any content decryption and fails closed on `false`.
pub fn verify_cek_commitment(node_set_id: &[u8], cek: &[u8], expected: &[u8; 32]) -> bool {
    use subtle::ConstantTimeEq;
    let got = cek_commitment(node_set_id, cek);
    got.ct_eq(expected).into()
}

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

/// Derive a domain-separated 32-byte sub-seed from a master seed:
/// `SHA-256(label ‖ master)`. Lets one persisted authority master seed deterministically
/// fan out into independent sub-keys (e.g. the seal signer seed vs the KEM recipient seed)
/// that can never be confused for one another. Pure, no RNG.
pub fn derive_seed(master: &[u8; 32], label: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(label);
    h.update(master);
    let digest = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..32]);
    out
}

/// Compute the domain-separated IDENTITY of a 2-of-2 threshold NODE-SET:
/// `SHA-256(DOMAIN ‖ [t] ‖ len(vk_a) ‖ vk_a ‖ len(vk_b) ‖ vk_b)`, with each `len` a 4-byte
/// big-endian length prefix so two distinct `(vk_a, vk_b)` pairs can never collide by concatenation.
/// This pins WHICH secret-holders back a threshold rail into a single stable id: a runtime that
/// provisions a node-set can durably record this id at publish, then RE-DERIVE it from the published
/// descriptor at open and fail closed if a node was silently swapped (the descriptor points at a
/// different secret-holder than the producer escrowed to). Pure, no RNG — the single source of truth
/// both the runtime (descriptor check) and any auditor share. Order matters: `(a,b) != (b,a)`.
pub fn threshold_node_set_id(t: u8, vk_a: &[u8], vk_b: &[u8]) -> [u8; 32] {
    threshold_node_set_id_n(t, &[vk_a, vk_b])
}

/// The n-NODE generalization of [`threshold_node_set_id`] (Day 113–116, the 2-of-3 quorum
/// rail): the same domain + encoding over an ORDERED list of node vks, so the 2-node id is
/// byte-identical to the original (no re-pinning on upgrade) and a t-of-n set pins ALL n
/// secret-holders — adding, removing, reordering, or swapping ANY member changes the id.
pub fn threshold_node_set_id_n(t: u8, vks: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"elastos.dkms.threshold.node-set/v1");
    h.update([t]);
    for vk in vks {
        h.update((vk.len() as u32).to_be_bytes());
        h.update(vk);
    }
    let digest = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..32]);
    out
}

/// Mint a fresh 32-byte master seed from the OS RNG (WASI `random_get` on
/// wasm32-wasip1). A durable key authority persists this ONCE and re-derives the same
/// keypairs forever via [`mint_session_from_seed`] / [`seal::mldsa_seal_keypair`].
pub fn random_seed() -> [u8; 32] {
    use rand_core::RngCore;
    let mut seed = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut seed);
    seed
}

/// DETERMINISTICALLY derive a hybrid KEM keypair from a 32-byte seed — the same seed
/// always yields byte-identical keys, with NO RNG. The key-authority analogue of
/// [`mint_session`]: a durable authority persists one master seed and re-derives the
/// SAME recipient key on every launch, so a producer can escrow a CEK to the published
/// recipient ONCE (at publish time) and any later authority launch resolves the identical
/// recipient. Domain-separated sub-seeds feed x25519 and ML-KEM-768 independently.
pub fn mint_session_from_seed(seed: [u8; 32]) -> (SessionKemSecret, SessionKemPublic) {
    use ml_kem::B32;
    let x_seed = derive_seed(&seed, b"elastos-session/x25519/v1");
    let d = derive_seed(&seed, b"elastos-session/ml-kem-d/v1");
    let z = derive_seed(&seed, b"elastos-session/ml-kem-z/v1");
    let x_sk = XStaticSecret::from(x_seed);
    let x_pk = XPublicKey::from(&x_sk);
    let d: B32 = d.into();
    let z: B32 = z.into();
    let (dk, ek) = MlKem768::generate_deterministic(&d, &z);
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

/// Domain label for the dKMS-authority node IDENTITY handshake (Day 89–90). A client pins the
/// node's published verifying key, sends a fresh random challenge, and the node returns a signature
/// over `DKMS_HELLO_DOMAIN ‖ challenge` proving it holds the master-derived signing key BEHIND that
/// vk — so a client can refuse to talk to an impersonated node before delegating any recovery. The
/// label is domain-separated from the CEK-seal signatures so a hello attestation can never be
/// replayed as a seal (or vice-versa). Defined ONCE here so the node + client cannot drift.
pub const DKMS_HELLO_DOMAIN: &[u8] = b"elastos.dkms.authority/hello/v1";

/// The NODE side of the identity handshake: sign `DKMS_HELLO_DOMAIN ‖ challenge` with the node's
/// master-derived seal signing key. Returns the detached signature (the attestation).
pub fn attest_challenge(signer: &impl seal::CekSealSigner, challenge: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(DKMS_HELLO_DOMAIN.len() + challenge.len());
    msg.extend_from_slice(DKMS_HELLO_DOMAIN);
    msg.extend_from_slice(challenge);
    signer.sign(&msg)
}

/// The CLIENT side of the identity handshake: verify a node's attestation over `challenge` under the
/// authority's PINNED verifying key. `true` only when the signature is a valid ML-DSA-65 signature
/// over `DKMS_HELLO_DOMAIN ‖ challenge` under `verifier` — a forged/mismatched node, a tampered
/// challenge, or a malformed signature all return `false` (the client then fails closed).
pub fn verify_attestation(verifier: &impl CekSealVerifier, challenge: &[u8], sig: &[u8]) -> bool {
    let mut msg = Vec::with_capacity(DKMS_HELLO_DOMAIN.len() + challenge.len());
    msg.extend_from_slice(DKMS_HELLO_DOMAIN);
    msg.extend_from_slice(challenge);
    verifier.verify(&msg, sig)
}

/// Domain label for the dKMS-authority SESSION TOKEN (Day 91–92). After the identity handshake, the
/// node issues a token binding the client's `challenge` to an `expires_at` and signs it with its
/// master-derived key; the node then REQUIRES a live, node-verified token on every `recover`, so a
/// long-lived node only recovers for a caller that completed the handshake IN THIS session (the
/// runtime-core analogue of PC2's per-view session resurrected to gate recovery,
/// `secureViewSession.ts:81`–`:128`). Domain-separated from the hello attestation + the CEK seals so
/// a token can never be replayed as either. Defined ONCE here so the node + client cannot drift.
pub const DKMS_SESSION_DOMAIN: &[u8] = b"elastos.dkms.authority/session/v1";

/// Domain label for the dKMS-authority RECOVER POSSESSION PROOF (Day 93–94). The session token is a
/// BEARER credential — anyone who captures the `hello` response holds it. To make it NON-REPLAYABLE
/// across callers, `hello` binds the token to a caller-minted ephemeral PUBLIC key, and every
/// `recover` must carry a signature (under the matching PRIVATE key) over this domain ‖ the session
/// challenge ‖ the recover binding; the node verifies it against the token-bound pubkey. A caller
/// who captured the token but lacks the private key cannot produce the proof → refused. The
/// runtime-core analogue of PC2's session being OWNER-BOUND (the bearer token alone is insufficient;
/// the owner is re-checked, in the TEE via `ecrecover(delegationSig)`, `secureViewSession.ts:87`–`:100`).
/// Domain-separated from the session token + hello attestation + CEK seals.
/// v2 (re-seal-AAD binding): the preimage now also binds `sha256(re-seal AAD)`, closing the
/// pre-mainnet invariant where the node sealed under a caller-supplied AAD it did not authenticate.
/// Bumped from v1 so a v1 proof can never be misread under v2 semantics.
pub const DKMS_RECOVER_DOMAIN: &[u8] = b"elastos.dkms.authority/recover-proof/v2";

/// Length-prefixed concatenation of variable-length fields into one unambiguous signed preimage:
/// each field is preceded by its u32(LE) length, so `("a","bc")` and `("ab","c")` never collide.
fn lp_concat(domain: &[u8], fields: &[&[u8]]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(domain.len() + fields.iter().map(|f| f.len() + 4).sum::<usize>());
    msg.extend_from_slice(domain);
    for f in fields {
        msg.extend_from_slice(&(f.len() as u32).to_le_bytes());
        msg.extend_from_slice(f);
    }
    msg
}

/// Canonical signed preimage of a session token: binds the client's `challenge`, the caller's
/// EPHEMERAL public key (`caller_pub`), AND the `expires_at` — tampering with any field invalidates
/// the signature, and binding `caller_pub` ties the bearer token to the key the caller must later
/// prove possession of.
fn session_token_message(challenge: &[u8], caller_pub: &[u8], expires_at: u64) -> Vec<u8> {
    lp_concat(DKMS_SESSION_DOMAIN, &[challenge, caller_pub, &expires_at.to_le_bytes()])
}

/// The NODE side: mint a session token by signing `(challenge, caller_pub, expires_at)` with the
/// node's master-derived signing key. Binding `caller_pub` is what makes the token non-replayable by
/// a caller who does not hold the matching private key. Returns the detached signature.
pub fn sign_session_token(
    signer: &impl seal::CekSealSigner,
    challenge: &[u8],
    caller_pub: &[u8],
    expires_at: u64,
) -> Vec<u8> {
    signer.sign(&session_token_message(challenge, caller_pub, expires_at))
}

/// Verify a session token's signature over `(challenge, caller_pub, expires_at)`. `true` only when
/// `sig` is a valid ML-DSA-65 signature under `verifier` — a forged token, or a tampered
/// challenge/caller_pub/expiry, or a malformed signature all return `false`. The node verifies tokens
/// under its OWN verifying key (it is the issuer); expiry is enforced separately against the clock.
pub fn verify_session_token(
    verifier: &impl CekSealVerifier,
    challenge: &[u8],
    caller_pub: &[u8],
    expires_at: u64,
    sig: &[u8],
) -> bool {
    verifier.verify(&session_token_message(challenge, caller_pub, expires_at), sig)
}

/// Canonical signed preimage of a recover possession proof: the session `challenge`, the
/// content/recipient binding of THIS recover (`content_id`, `kid_hex`, the decrypt session pubkey),
/// a per-recover FRESHNESS counter (`recover_seq`, Day 95–96), AND `sha256(re-seal AAD)` (v2). Binding
/// the recover identity means the proof authorizes recovering THIS content for THIS session; binding a
/// strictly-increasing `recover_seq` means a captured recover frame replayed verbatim carries a STALE
/// counter the node has already consumed, so it is refused (anti-replay); binding the re-seal AAD
/// digest means the node will only seal under the EXACT AAD the caller proved possession over — a
/// MITM-tampered `aad_b64` invalidates the proof and is refused at the node (the AAD itself carries
/// `node_set_id` + `segment_digests`, so all of them are bound transitively). We bind the 32-byte
/// digest, not the raw AAD, so a long presentation's segment digests don't bloat the preimage. The
/// runtime-core analogue of PC2's per-delegation revocable `nonce` (`secureViewSession.ts:108`–`:112`).
/// Defined ONCE here so the node + client cannot drift.
pub fn recover_proof_message(
    challenge: &[u8],
    content_id: &[u8],
    kid_hex: &[u8],
    decrypt_session_pub: &[u8],
    recover_seq: u64,
    reseal_aad: &[u8],
) -> Vec<u8> {
    let aad_digest = Sha256::digest(reseal_aad);
    lp_concat(
        DKMS_RECOVER_DOMAIN,
        &[
            challenge,
            content_id,
            kid_hex,
            decrypt_session_pub,
            &recover_seq.to_le_bytes(),
            &aad_digest[..],
        ],
    )
}

/// The CLIENT side: prove possession of the token-bound ephemeral private key by signing the recover
/// binding + this recover's freshness counter + the re-seal AAD digest. The node verifies this against
/// the pubkey the session token committed to AND that `recover_seq` strictly advances AND that the AAD
/// it is about to seal under matches the one signed here (a replayed or AAD-tampered frame is refused).
pub fn sign_recover_proof(
    signer: &impl seal::CekSealSigner,
    challenge: &[u8],
    content_id: &[u8],
    kid_hex: &[u8],
    decrypt_session_pub: &[u8],
    recover_seq: u64,
    reseal_aad: &[u8],
) -> Vec<u8> {
    signer.sign(&recover_proof_message(
        challenge,
        content_id,
        kid_hex,
        decrypt_session_pub,
        recover_seq,
        reseal_aad,
    ))
}

/// The NODE side: verify the caller's possession proof against the token-bound pubkey. `true` only
/// when `sig` is valid under `verifier` (built from the token's `caller_pub`) over the SAME binding
/// (including `recover_seq` and `sha256(reseal_aad)`) — a missing/forged proof, a proof from a
/// different key, a tampered binding, a swapped freshness counter, or a tampered re-seal AAD all
/// return `false`. Freshness (the strictly-increasing check) is enforced by the node against its
/// per-session counter, not here.
pub fn verify_recover_proof(
    verifier: &impl CekSealVerifier,
    challenge: &[u8],
    content_id: &[u8],
    kid_hex: &[u8],
    decrypt_session_pub: &[u8],
    recover_seq: u64,
    reseal_aad: &[u8],
    sig: &[u8],
) -> bool {
    verifier.verify(
        &recover_proof_message(
            challenge,
            content_id,
            kid_hex,
            decrypt_session_pub,
            recover_seq,
            reseal_aad,
        ),
        sig,
    )
}

/// Domain label for a QUORUM RELEASE ATTESTATION (Day 131–135): every secret-holder that serves a
/// threshold open CO-SIGNS a portable proof that *it* authorized *this* content for *this* principal
/// under *this* decrypt session. The boundary aggregates the t co-signatures into a
/// [`QuorumReleaseProofV1`]-style bundle that a THIRD PARTY can verify OFFLINE — no runtime, no
/// secrets, no live node — to confirm WHICH node-set served the open, that a real quorum (≥ t
/// DISTINCT members) signed, that it is bound to that exact principal+content+session, and that it
/// has not expired. Today an open persists a CEK-free record the runtime writes ABOUT ITSELF; a
/// relying party still has to trust the runtime authored it faithfully. This closes that gap: the
/// evidence is signed by the secret-holders THEMSELVES, so its authenticity does not depend on the
/// runtime. Domain-separated from the hello/session/recover/seal labels so a release attestation can
/// never be replayed as any of them (or vice-versa). (PC2 has no analogue: its open emits no
/// portable, independently-verifiable proof of WHICH nodes served it — the Lit network is opaque.)
/// Defined ONCE here so every node + every verifier compute byte-identical preimages.
pub const DKMS_RELEASE_ATTEST_DOMAIN: &[u8] = b"elastos.dkms.authority/release-attestation/v1";

/// Canonical signed preimage of a single node's release attestation:
/// `DKMS_RELEASE_ATTEST_DOMAIN ‖ lp(content_id) ‖ lp(principal_id) ‖ lp(right) ‖ lp(node_set_id) ‖
/// lp(decrypt_session_pub) ‖ lp(kid16) ‖ lp(expiry_le)`. Every field is bound: the grant
/// (`content_id`, `principal_id`, `right`), the node-set the signer claims membership of
/// (`node_set_id`), the per-open freshness (`decrypt_session_pub` — a fresh ephemeral key per open,
/// so a captured attestation cannot be replayed against a DIFFERENT open), the key id (`kid16`), and
/// the `expiry`. ALL quorum members sign byte-identical preimages, which is what lets the boundary
/// aggregate their signatures into one portable proof.
pub fn release_attestation_message(
    content_id: &[u8],
    principal_id: &[u8],
    right: &[u8],
    node_set_id: &[u8],
    decrypt_session_pub: &[u8],
    kid_bytes16: &[u8; 16],
    expiry: u64,
) -> Vec<u8> {
    lp_concat(
        DKMS_RELEASE_ATTEST_DOMAIN,
        &[
            content_id,
            principal_id,
            right,
            node_set_id,
            decrypt_session_pub,
            kid_bytes16,
            &expiry.to_le_bytes(),
        ],
    )
}

/// The NODE side: co-sign a release attestation for THIS open with the node's master-derived signing
/// key (the same identity behind the vk pinned in the descriptor). Returns the detached signature
/// the boundary collects from every releasing member.
#[allow(clippy::too_many_arguments)]
pub fn sign_release_attestation(
    signer: &impl seal::CekSealSigner,
    content_id: &[u8],
    principal_id: &[u8],
    right: &[u8],
    node_set_id: &[u8],
    decrypt_session_pub: &[u8],
    kid_bytes16: &[u8; 16],
    expiry: u64,
) -> Vec<u8> {
    signer.sign(&release_attestation_message(
        content_id,
        principal_id,
        right,
        node_set_id,
        decrypt_session_pub,
        kid_bytes16,
        expiry,
    ))
}

/// Verify ONE node's release attestation under its verifying key. `true` only when `sig` is a valid
/// ML-DSA-65 signature over the EXACT binding — a forged signature, a tampered grant/session/expiry,
/// or a signature from the wrong key all return `false`.
#[allow(clippy::too_many_arguments)]
pub fn verify_release_attestation(
    verifier: &impl CekSealVerifier,
    content_id: &[u8],
    principal_id: &[u8],
    right: &[u8],
    node_set_id: &[u8],
    decrypt_session_pub: &[u8],
    kid_bytes16: &[u8; 16],
    expiry: u64,
    sig: &[u8],
) -> bool {
    verifier.verify(
        &release_attestation_message(
            content_id,
            principal_id,
            right,
            node_set_id,
            decrypt_session_pub,
            kid_bytes16,
            expiry,
        ),
        sig,
    )
}

/// Why a [`verify_quorum_release_proof`] check failed — every variant FAILS CLOSED and, where an
/// individual member is at fault, NAMES it by its index in the ordered member list so a relying
/// party can attribute the bad attestation to a specific node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuorumProofError {
    /// The proof carries no member vks — nothing to verify against.
    EmptyMembers,
    /// The `node_set_id` the proof claims does not equal the id RECOMPUTED from `(t, members)` — the
    /// proof is lying about WHICH node-set it represents (or the member list was tampered).
    NodeSetMismatch,
    /// A signature references a member index outside the member list, or a member vk is malformed.
    MalformedMember { member_index: usize },
    /// The same member index signed twice — a quorum requires t DISTINCT members; a duplicate cannot
    /// pad the count.
    DuplicateSigner { member_index: usize },
    /// A member's signature does not verify over the bound grant/session/expiry — that member did not
    /// authorize THIS open (forged, redirected, or bound to different fields). Names the member.
    BadSignature { member_index: usize },
    /// Fewer than `t` DISTINCT members produced valid signatures — not a real quorum.
    BelowQuorum { have: usize, need: usize },
    /// `now` is past the attested `expiry` — the proof has aged out.
    Expired,
}

/// The STANDALONE, OFFLINE verifier for an aggregated quorum release proof (Day 131–135) — the heart
/// of "the quorum PROVES it served you". Given the proof's `(t, members, node_set_id)`, the grant +
/// session + expiry the relying party EXPECTS, the current time, and the collected
/// `(member_index, signature)` pairs, it confirms, with NO runtime and NO secrets:
///
/// 1. the proof represents a real, NAMED node-set — `node_set_id` must equal
///    `threshold_node_set_id_n(t, members)`, so a proof cannot claim a set it isn't (and after a
///    reconfiguration a proof names the CURRENT set, since its members + t recompute the live id);
/// 2. at least `t` DISTINCT members signed — duplicates are refused, so an under-quorum bundle (or a
///    single node replaying its own signature t times) is rejected;
/// 3. EVERY counted signature verifies over the binding the relying party expects — so a proof minted
///    for principal A / content X / session S does NOT verify when checked for a different principal,
///    content, or session (the signatures were never over those bytes), and a forged member
///    signature is rejected AND that member is named;
/// 4. the proof has not expired (`now <= expiry`).
///
/// Returns the count of valid DISTINCT signers on success. Note the binding is supplied by the
/// CALLER, not read from the proof's self-description: the verifier confirms the quorum signed
/// EXACTLY the grant/session the relying party cares about, which is what makes "wrong-principal" and
/// "replayed-against-another-open" attempts fail closed.
#[allow(clippy::too_many_arguments)]
pub fn verify_quorum_release_proof(
    t: u8,
    members: &[&[u8]],
    node_set_id: &[u8],
    content_id: &[u8],
    principal_id: &[u8],
    right: &[u8],
    decrypt_session_pub: &[u8],
    kid_bytes16: &[u8; 16],
    expiry: u64,
    now: u64,
    signatures: &[(usize, &[u8])],
) -> Result<usize, QuorumProofError> {
    if members.is_empty() {
        return Err(QuorumProofError::EmptyMembers);
    }
    // (1) The proof must NAME the node-set it represents: recompute the id from the ordered members +
    // threshold and refuse any mismatch BEFORE trusting a single signature.
    let recomputed = threshold_node_set_id_n(t, members);
    if recomputed.as_slice() != node_set_id {
        return Err(QuorumProofError::NodeSetMismatch);
    }
    // (4) Expiry is a cheap, signer-independent gate — check it before signature work.
    if now > expiry {
        return Err(QuorumProofError::Expired);
    }
    // (2)+(3) Count DISTINCT members whose signature verifies over the EXPECTED binding.
    let mut seen: Vec<usize> = Vec::with_capacity(signatures.len());
    let mut valid = 0usize;
    for (idx, sig) in signatures {
        let idx = *idx;
        if idx >= members.len() {
            return Err(QuorumProofError::MalformedMember { member_index: idx });
        }
        if seen.contains(&idx) {
            return Err(QuorumProofError::DuplicateSigner { member_index: idx });
        }
        seen.push(idx);
        let verifier = match MlDsa65Verifier::from_encoded(members[idx]) {
            Some(v) => v,
            None => return Err(QuorumProofError::MalformedMember { member_index: idx }),
        };
        if !verify_release_attestation(
            &verifier,
            content_id,
            principal_id,
            right,
            node_set_id,
            decrypt_session_pub,
            kid_bytes16,
            expiry,
            sig,
        ) {
            return Err(QuorumProofError::BadSignature { member_index: idx });
        }
        valid += 1;
    }
    if valid < t as usize {
        return Err(QuorumProofError::BelowQuorum { have: valid, need: t as usize });
    }
    Ok(valid)
}

/// Domain label for the dKMS ENCRYPTED-CHANNEL key attestation (Day 105–108). When the node is
/// reached over a NETWORK transport (TCP), the client and node establish an app-layer encrypted,
/// mutually-authenticated channel: at `hello` the node publishes a master-derived CHANNEL KEM key
/// and signs `(challenge ‖ channel_pub)` under its pinned identity. Binding the channel key INTO the
/// identity attestation is what defeats an attacker terminating the TCP connection: it could relay
/// the node's genuine hello, but substituting its OWN KEM key breaks this signature under the
/// descriptor-pinned vk, so the client refuses the channel. (PC2 has no analogue — its dDRM fetch
/// runs HTTPS with `rejectUnauthorized: false`, `chipotle-client.ts:840`: the TLS layer authenticates
/// NOTHING and only the payload signature saves it.) Domain-separated from the hello attestation so
/// neither signature can stand in for the other. Defined ONCE here so node + client cannot drift.
pub const DKMS_CHANNEL_DOMAIN: &[u8] = b"elastos.dkms.authority/channel-key/v1";

/// The NODE side: attest the channel KEM key for THIS handshake — sign
/// `DKMS_CHANNEL_DOMAIN ‖ lp(challenge) ‖ lp(channel_pub)` with the node's master-derived signing key.
pub fn attest_channel_key(
    signer: &impl seal::CekSealSigner,
    challenge: &[u8],
    channel_pub: &[u8],
) -> Vec<u8> {
    signer.sign(&lp_concat(DKMS_CHANNEL_DOMAIN, &[challenge, channel_pub]))
}

/// The CLIENT side: verify the node's channel-key attestation under the PINNED identity. `true`
/// only for a valid signature over the SAME `(challenge, channel_pub)` — a substituted KEM key
/// (a MITM terminating the TCP connection), a replayed challenge, or a forged signature all return
/// `false` and the client refuses to establish the channel (fail-closed: no channel, no recover).
pub fn verify_channel_key(
    verifier: &impl CekSealVerifier,
    challenge: &[u8],
    channel_pub: &[u8],
    sig: &[u8],
) -> bool {
    verifier.verify(&lp_concat(DKMS_CHANNEL_DOMAIN, &[challenge, channel_pub]), sig)
}

/// Domain label for the per-FRAME AAD of an established dKMS encrypted channel (Day 105–108).
/// Every frame body on the channel is a sealed envelope whose AEAD is bound to
/// `(channel_id, direction, seq)`: `channel_id` scopes the frame to THIS handshake (the hello
/// challenge), `direction` (0 = client→node, 1 = node→client) makes a frame non-REFLECTABLE back at
/// its sender, and the strictly-advancing `seq` makes a captured frame non-REPLAYABLE on the same
/// channel (the receiver's counter has moved on, so the AAD no longer matches and the AEAD open
/// fails). Domain-separated from every other AAD in the system.
pub const DKMS_CHANNEL_FRAME_DOMAIN: &[u8] = b"elastos.dkms.authority/channel-frame/v1";

/// Canonical per-frame channel AAD: `DKMS_CHANNEL_FRAME_DOMAIN ‖ lp(channel_id) ‖ lp(direction) ‖
/// lp(seq_le)`. Defined ONCE here so the node + every client compute byte-identical AADs (a drifted
/// encoder would fail every AEAD open, fail-closed).
pub fn channel_frame_aad(channel_id: &[u8], direction: u8, seq: u64) -> Vec<u8> {
    lp_concat(DKMS_CHANNEL_FRAME_DOMAIN, &[channel_id, &[direction], &seq.to_le_bytes()])
}

/// Domain label for a SHARE-WISE node-set ROTATION (Day 109–112): the operator instructs a
/// secret-holding node to re-escrow ITS share to a successor node, refreshed by an XOR delta — the
/// whole CEK is NEVER reassembled anywhere during rotation. The refresh delta travels as a sealed
/// envelope to the rotating node, AEAD-bound to THIS AAD: the kid being rotated, the SOURCE node's
/// escrow recipient (only that node can open it) and the SUCCESSOR's recipient (a MITM cannot
/// redirect the rotated share to its own recipient — the AAD would not match). Signed by the
/// OPERATOR identity the node pins at daemon start, so only the operator can authorize a rotation.
/// (PC2 has no analogue at all: its "rotation" is a manual constant redeploy with NO migration of
/// existing content — `chipotle-client.ts:125`/`:1043`/`:1064`.) Defined ONCE here so the runtime
/// and the node cannot drift.
pub const DKMS_ROTATE_DOMAIN: &[u8] = b"elastos.dkms.authority/rotate-share/v1";

/// Canonical rotation AAD: `DKMS_ROTATE_DOMAIN ‖ lp(kid16) ‖ lp(source_recipient_pub) ‖
/// lp(successor_recipient_pub)`. The operator seals the refresh delta under it; the rotating node
/// recomputes it from its OWN recipient key + the request's successor — a delta sealed for a
/// different kid, a different source node, or a different successor fails the AEAD open.
pub fn rotation_aad(
    kid_bytes16: &[u8; 16],
    source_recipient_pub: &[u8],
    successor_recipient_pub: &[u8],
) -> Vec<u8> {
    lp_concat(DKMS_ROTATE_DOMAIN, &[kid_bytes16, source_recipient_pub, successor_recipient_pub])
}

/// Domain label for a quorum RECONFIGURATION (Day 121–125): the operator instructs a live k-of-m
/// re-sharing — each OLD quorum member sub-shares its share under a fresh degree-(k−1) polynomial
/// and each NEW node combines the sub-shares into its new share, so the threshold AND the membership
/// change while the CEK is never reassembled. Domain-separated from the share-wise ROTATION
/// (`DKMS_ROTATE_DOMAIN`) so a rotation delta can never be replayed as a reconfiguration instruction
/// or vice-versa. (PC2 has no analogue at all — its t-of-n is Lit's opaque network, whose t, n, and
/// membership are fixed and uninspectable; there is no protocol to change them.) Defined ONCE here
/// so the operator, the contributing nodes, and the new nodes cannot drift.
pub const DKMS_RESHARE_DOMAIN: &[u8] = b"elastos.dkms.authority/reshare/v1";

/// Canonical reconfiguration AAD: `DKMS_RESHARE_DOMAIN ‖ lp(kid16) ‖ lp(old_node_set_id) ‖
/// lp(new_node_set_id) ‖ lp([k]) ‖ lp([m])`. The operator seals the re-sharing authorization bound
/// to the EXACT (kid, old set, new set, new threshold k, new size m): an instruction minted for one
/// reconfiguration cannot be replayed against another kid, redirected to a different new set, or
/// silently DOWNGRADED to a smaller k — every field is welded into the AEAD. `old_node_set_id` /
/// `new_node_set_id` are the [`threshold_node_set_id_n`] hashes pinning each membership + threshold.
pub fn reshare_aad(
    kid_bytes16: &[u8; 16],
    old_node_set_id: &[u8],
    new_node_set_id: &[u8],
    k: u8,
    m: u8,
) -> Vec<u8> {
    lp_concat(DKMS_RESHARE_DOMAIN, &[kid_bytes16, old_node_set_id, new_node_set_id, &[k], &[m]])
}

/// Domain label for a single RECONFIGURATION SUB-SHARE — the sealed `q_i(y_j)` an OLD contributor
/// `i` routes to a NEW node `j` during a re-share. Separated from the reconfiguration AUTHORIZATION
/// (`DKMS_RESHARE_DOMAIN`) so an operator-authorization envelope can never be unwrapped as a
/// sub-share or vice-versa.
pub const DKMS_RESHARE_SUBSHARE_DOMAIN: &[u8] = b"elastos.dkms.authority/reshare-subshare/v1";

/// Canonical sub-share AAD: `DKMS_RESHARE_SUBSHARE_DOMAIN ‖ lp(kid16) ‖ lp(new_node_set_id) ‖
/// lp([contributor_x]) ‖ lp([target_x])`. The contributing node seals `contributor_x ‖ q_i(y_j)`
/// to new node `j` bound to this AAD; the new node re-derives the SAME AAD and verifies under the
/// contributor's identity, so a sub-share minted for one (contributor → target) pair cannot be
/// redirected to a different new node, replayed across reconfigurations, or have its contributor
/// coordinate forged (the coordinate that determines its Lagrange weight is welded into the AEAD).
pub fn reshare_subshare_aad(
    kid_bytes16: &[u8; 16],
    new_node_set_id: &[u8],
    contributor_x: u8,
    target_x: u8,
) -> Vec<u8> {
    lp_concat(
        DKMS_RESHARE_SUBSHARE_DOMAIN,
        &[kid_bytes16, new_node_set_id, &[contributor_x], &[target_x]],
    )
}

/// Domain label for a DISTRIBUTED KEY GENERATION ceremony (Day 126–130): the operator authorizes a
/// fresh t-of-m key to be BORN distributed across a node-set — each member acts as a dealer drawing a
/// random degree-(t−1) polynomial, the CEK is the sum of the dealers' constant terms, and no member
/// ever holds it. Domain-separated from RECONFIGURATION (`DKMS_RESHARE_DOMAIN`) so a DKG instruction
/// can never be replayed as a re-share or vice-versa. (PC2 has no analogue: a Lit key is generated
/// inside Lit's network with the dealer set, threshold, and refresh policy all opaque and immutable.)
pub const DKMS_DKG_DOMAIN: &[u8] = b"elastos.dkms.authority/dkg/v1";

/// Canonical DKG authorization AAD: `DKMS_DKG_DOMAIN ‖ lp(kid16) ‖ lp(dkg_id) ‖ lp(node_set_id) ‖
/// lp([t]) ‖ lp([m])`. The operator seals the contribute/install authorization bound to the EXACT
/// (kid, ceremony id, membership+threshold node-set id, threshold t, size m): an instruction minted
/// for one ceremony cannot be replayed against another kid, redirected to a different node-set, or
/// downgraded to a smaller t. `node_set_id` is the [`threshold_node_set_id_n`] hash pinning the m
/// dealers + the threshold; `dkg_id` is a fresh per-ceremony nonce.
pub fn dkg_aad(
    kid_bytes16: &[u8; 16],
    dkg_id: &[u8],
    node_set_id: &[u8],
    t: u8,
    m: u8,
) -> Vec<u8> {
    lp_concat(DKMS_DKG_DOMAIN, &[kid_bytes16, dkg_id, node_set_id, &[t], &[m]])
}

/// Domain label for a single DKG SUB-SHARE — the sealed `f_i(x_j)` a DEALER `i` routes to member `j`
/// during a ceremony. Separated from the DKG AUTHORIZATION (`DKMS_DKG_DOMAIN`) and from the
/// reconfiguration sub-share (`DKMS_RESHARE_SUBSHARE_DOMAIN`) so the three envelope kinds can never be
/// cross-unwrapped.
pub const DKMS_DKG_SUBSHARE_DOMAIN: &[u8] = b"elastos.dkms.authority/dkg-subshare/v1";

/// Canonical DKG sub-share AAD: `DKMS_DKG_SUBSHARE_DOMAIN ‖ lp(kid16) ‖ lp(dkg_id) ‖
/// lp(node_set_id) ‖ lp([dealer_x]) ‖ lp([target_x])`. The dealing node seals `dealer_x ‖ f_i(x_j)`
/// to member `j` bound to this AAD; member `j` re-derives the SAME AAD and unwraps under the dealer's
/// identity — so a tampered, forged, or REDIRECTED sub-share is refused and the DEALER (the signer)
/// is NAMED. The dealer coordinate is welded in so a sub-share minted by one dealer cannot be passed
/// off as another's, and the ceremony id + node-set id stop cross-ceremony replay.
pub fn dkg_subshare_aad(
    kid_bytes16: &[u8; 16],
    dkg_id: &[u8],
    node_set_id: &[u8],
    dealer_x: u8,
    target_x: u8,
) -> Vec<u8> {
    lp_concat(
        DKMS_DKG_SUBSHARE_DOMAIN,
        &[kid_bytes16, dkg_id, node_set_id, &[dealer_x], &[target_x]],
    )
}

/// Domain label for an OPERATOR-signed caller REVOCATION (Day 109–112): the node removes a caller
/// from service at runtime — its next `hello` AND any `recover` under a still-live session token are
/// refused (revocation outranks a live session). The runtime-core analogue of PC2 revoking a
/// delegation nonce that the TEE reads back per request (`secureViewSession.ts:108`–`:112`,
/// `revokeDelegation`/`isDelegationRevoked` in `utils/secureViewSession.ts:382`–`:399`) — except
/// here the SIGNED instruction reaches the key-holding node itself, not just an HTTP middleware.
/// Domain-separated from every other signature in the system.
pub const DKMS_REVOKE_DOMAIN: &[u8] = b"elastos.dkms.authority/revoke-caller/v1";

/// The OPERATOR side: sign a revocation of `caller_pub` (the caller's ML-DSA verifying key).
pub fn sign_revocation(signer: &impl seal::CekSealSigner, caller_pub: &[u8]) -> Vec<u8> {
    signer.sign(&lp_concat(DKMS_REVOKE_DOMAIN, &[caller_pub]))
}

/// The NODE side: verify a revocation under the operator identity pinned at daemon start. `true`
/// only for a valid operator signature over the SAME caller key — a forged signature or a
/// signature lifted from another domain (hello attestation, channel attestation, session token)
/// returns `false` and the revocation is refused.
pub fn verify_revocation(verifier: &impl CekSealVerifier, caller_pub: &[u8], sig: &[u8]) -> bool {
    verifier.verify(&lp_concat(DKMS_REVOKE_DOMAIN, &[caller_pub]), sig)
}

/// Length-prefixed message FRAMING for the dKMS node's socket transport (Day 93–94): every message
/// is `[4-byte length (BE)][payload]`, so a reader recovers exact message boundaries instead of
/// trusting a raw byte stream. The runtime-core analogue of PC2's Boson proxy framing —
/// `[2-byte length (BE, includes itself)][1-byte type][body]`, `MAX_PACKET_SIZE`, `PACKET_HEADER_SIZE`
/// (`ProxyProtocol.ts:13`/`:251`/`:256`/`:371`). Defined ONCE here so the node + every client agree
/// on the wire. Fail-closed: an oversized length is refused before allocating, and a torn/half frame
/// is an error (never a partial parse).
pub mod frame {
    use std::io::{self, Read, Write};

    /// Maximum framed payload (1 MiB). A dKMS request/response is small JSON; anything larger is a
    /// torn/hostile frame and is refused before allocating a buffer for it.
    pub const MAX_FRAME_BYTES: u32 = 1 << 20;

    /// Write one length-prefixed frame: `[4-byte BE length][payload]`, then flush. An oversized
    /// payload (or one that does not fit a `u32`) is refused fail-closed.
    pub fn write_frame(w: &mut impl Write, payload: &[u8]) -> io::Result<()> {
        let len = u32::try_from(payload.len())
            .ok()
            .filter(|n| *n <= MAX_FRAME_BYTES)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "frame payload exceeds MAX_FRAME_BYTES"))?;
        w.write_all(&len.to_be_bytes())?;
        w.write_all(payload)?;
        w.flush()
    }

    /// Read one length-prefixed frame. Returns `Ok(Some(payload))` for a complete frame, or
    /// `Ok(None)` for a CLEAN end-of-stream at a frame boundary (the peer hung up between messages —
    /// a normal half-close). ERRORS fail-closed for an oversized length, a zero length, or a TORN
    /// frame (EOF mid-header or mid-payload) — never a partial/ambiguous parse.
    pub fn read_frame(r: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
        let mut header = [0u8; 4];
        match read_exact_or_eof(r, &mut header)? {
            ReadState::CleanEof => return Ok(None),
            ReadState::Torn => {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "torn frame header"))
            }
            ReadState::Full => {}
        }
        let len = u32::from_be_bytes(header);
        if len == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "zero-length frame"));
        }
        if len > MAX_FRAME_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "frame length exceeds MAX_FRAME_BYTES"));
        }
        let mut payload = vec![0u8; len as usize];
        match read_exact_or_eof(r, &mut payload)? {
            ReadState::Full => Ok(Some(payload)),
            // A header that promised N bytes but the stream ended early is a torn frame, never a
            // short read we silently accept.
            _ => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "torn frame payload")),
        }
    }

    enum ReadState {
        Full,
        CleanEof,
        Torn,
    }

    /// Fill `buf` fully; distinguish a clean EOF AT THE START (no bytes read) from a torn read
    /// (some-but-not-all bytes read) from a full read.
    fn read_exact_or_eof(r: &mut impl Read, buf: &mut [u8]) -> io::Result<ReadState> {
        let mut filled = 0;
        while filled < buf.len() {
            match r.read(&mut buf[filled..]) {
                Ok(0) => return Ok(if filled == 0 { ReadState::CleanEof } else { ReadState::Torn }),
                Ok(n) => filled += n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(ReadState::Full)
    }
}

/// Length-hiding PADDING for the dKMS encrypted-channel frames (pre-audit #5: metadata
/// minimization). Padding is applied to the PLAINTEXT *before* sealing, so the sealed envelope —
/// and therefore the `[len][payload]` frame an on-path observer sees — lands on a coarse size BUCKET
/// instead of revealing the exact message size (which would distinguish e.g. a `status` poll from a
/// `recover`). The recipient strips the padding *after* opening the authenticated envelope, where the
/// length is integrity-protected. This is a COARSE defense: it collapses the small control/recover
/// messages that matter most, but it is not full traffic-analysis resistance — the deep fix
/// (blinded identifiers / oblivious lookup) is roadmap. See docs/THREAT_MODEL.md.
///
/// Layout (ISO/IEC 7816-4): `plaintext ‖ 0x80 ‖ 0x00*k`. The `0x80` marker is ALWAYS present, so
/// `unpad` is unambiguous even when the plaintext itself ends in `0x00`.
pub mod channel_pad {
    /// Smallest bucket — every control message rounds up to at least this.
    const MIN_BUCKET: usize = 256;
    /// Largest bucketed size. Above this we add the marker only (no size-class expansion): such a
    /// frame is a rare large content-binding `recover` whose size is already dominated by its
    /// payload, and capping here keeps a padded frame far below `frame::MAX_FRAME_BYTES` even after
    /// the seal overhead, so padding can never tip a frame over the transport cap.
    const TOP_BUCKET: usize = 128 * 1024;

    /// The padded length for a plaintext of `n` bytes (including the mandatory marker byte): powers of
    /// two from `MIN_BUCKET` up to `TOP_BUCKET`, marker-only beyond.
    fn bucket(n: usize) -> usize {
        if n <= MIN_BUCKET {
            MIN_BUCKET
        } else if n <= TOP_BUCKET {
            n.next_power_of_two()
        } else {
            n
        }
    }

    /// Whether to ADD length-hiding padding to OUTGOING channel frames. **OFF by default.**
    ///
    /// Padding changes the dKMS channel wire format: an un-upgraded quorum node cannot parse a
    /// padded plaintext, so a client that pads unilaterally breaks interop with the deployed nodes.
    /// Padding is therefore a NEGOTIATED feature — enable it with `ELASTOS_DKMS_CHANNEL_PAD=1` on
    /// BOTH the client and the node only once every node in the set ships a padding-aware build.
    /// The RECEIVER (`unpad_incoming`) is ALWAYS tolerant of both wire forms, so flipping this on or
    /// off is rollout-safe in any order. (Padding is metadata minimization, not an integrity
    /// boundary — the authenticated seal already protects these bytes.)
    pub fn enabled() -> bool {
        use std::sync::OnceLock;
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| {
            std::env::var("ELASTOS_DKMS_CHANNEL_PAD")
                .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false)
        })
    }

    /// Pad `plaintext` to its size bucket. Always grows by at least the one marker byte.
    pub fn pad(plaintext: &[u8]) -> Vec<u8> {
        let target = bucket(plaintext.len() + 1);
        let mut out = Vec::with_capacity(target);
        out.extend_from_slice(plaintext);
        out.push(0x80);
        out.resize(target, 0x00);
        out
    }

    /// Wire-side outgoing transform: pad ONLY when channel padding is enabled, else emit the
    /// plaintext unchanged so the frame matches the un-padded wire format the deployed nodes speak.
    pub fn pad_outgoing(plaintext: &[u8]) -> Vec<u8> {
        if enabled() {
            pad(plaintext)
        } else {
            plaintext.to_vec()
        }
    }

    /// Strip ISO 7816-4 padding: drop trailing `0x00`, then the single `0x80` marker. Returns `None`
    /// for a malformed pad (no marker, or all-zero). Used directly only by the round-trip tests.
    pub fn unpad(padded: &[u8]) -> Option<Vec<u8>> {
        let mut i = padded.len();
        while i > 0 && padded[i - 1] == 0x00 {
            i -= 1;
        }
        if i == 0 || padded[i - 1] != 0x80 {
            return None;
        }
        Some(padded[..i - 1].to_vec())
    }

    /// Wire-side incoming transform: if the frame carries a valid ISO 7816-4 pad marker, strip it;
    /// otherwise return it unchanged (the peer did not pad). This makes a receiver accept BOTH wire
    /// forms, so a padded and an un-padded peer interoperate regardless of rollout order. It is safe
    /// because these channel payloads are JSON: valid JSON never ends in a `0x80`-marked, zero-padded
    /// tail, so "padded" vs "raw" is unambiguous — and the seal already authenticated the bytes, so
    /// this layer is not an integrity gate.
    pub fn unpad_incoming(frame: &[u8]) -> Vec<u8> {
        match unpad(frame) {
            Some(inner) => inner,
            None => frame.to_vec(),
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
    fn grant_watermark_digest_is_deterministic_and_normalised() {
        let a = grant_watermark_digest16("0x1234ABCD");
        // Deterministic.
        assert_eq!(a, grant_watermark_digest16("0x1234ABCD"));
        // Case- and whitespace-insensitive (re-serialisation can't break the embedder↔verifier match).
        assert_eq!(a, grant_watermark_digest16("  0x1234abcd  "));
        // Distinct inputs ⇒ distinct anchors (different wallet signatures don't collide).
        assert_ne!(a, grant_watermark_digest16("0x1234abce"));
    }

    /// GOLDEN cross-check shared with `elastos-server`'s no-shared-dep twin
    /// (`grant_watermark_digest16_hex`, asserted there against the SAME `(sig → digest)` pair). These
    /// two crates do not share a dependency, so this pinned vector is what keeps them from drifting: a
    /// change to the hashing OR the trim/lowercase normalization on either side fails one assertion.
    #[test]
    fn grant_watermark_digest_golden_vector() {
        const GOLDEN: &str = "a9e8be55b175d58849e16689d09a746f";
        let hex: String = grant_watermark_digest16("0x1234abcd")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(hex, GOLDEN);
        // Same digest under mixed case + whitespace (pins the normalization the twin must match).
        let norm: String = grant_watermark_digest16("  0x1234ABCD  ")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(norm, GOLDEN);
    }

    #[test]
    fn mint_session_from_seed_is_deterministic_and_usable() {
        let seed = [0x4Du8; 32];
        // Same seed → byte-identical published recipient key, every time.
        let (_s1, p1) = mint_session_from_seed(seed);
        let (s2, p2) = mint_session_from_seed(seed);
        assert_eq!(
            session_public_bytes(&p1),
            session_public_bytes(&p2),
            "a stable authority re-derives the SAME recipient key on every launch"
        );
        // A different seed → a different recipient.
        let (_s3, p3) = mint_session_from_seed([0x4Eu8; 32]);
        assert_ne!(session_public_bytes(&p1), session_public_bytes(&p3));
        // The re-derived recipient still recovers a CEK escrowed to it (the escrow-at-publish
        // path): seal to p1's published key, unwrap with a FRESH re-derivation of the secret.
        let (signer, vk) = mldsa_seal_keypair(SEED);
        let verifier = MlDsa65Verifier::from_encoded(&vk).expect("verifier");
        let aad = b"escrow:kid=abc";
        let env = seal_bound(&p1, &cek(), aad, &signer);
        let recovered = hybrid_unwrap_bound(&s2, &env, aad, &verifier).expect("open with re-derived secret");
        assert_eq!(recovered.as_slice(), cek().as_slice());
    }

    #[test]
    fn derive_seed_is_deterministic_and_domain_separated() {
        let master = [0x11u8; 32];
        assert_eq!(derive_seed(&master, b"seal"), derive_seed(&master, b"seal"));
        // Different labels → independent sub-seeds (can't be confused for one another).
        assert_ne!(derive_seed(&master, b"seal"), derive_seed(&master, b"recipient"));
        // Different master → different sub-seed under the same label.
        assert_ne!(derive_seed(&master, b"seal"), derive_seed(&[0x12u8; 32], b"seal"));
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

    // --- shared decrypt transcript (the AAD both sides must agree on) ----------

    fn sample_transcript() -> crate::transcript::DecryptTranscriptV1<'static> {
        crate::transcript::DecryptTranscriptV1 {
            suite_id: SUITE_PQ_HYBRID,
            provider_id: "decrypt-provider",
            principal_id: "did:elastos:alice",
            session_id: "sess-1",
            object_cid: "bafyobject",
            content_hash: b"content-hash-32-bytes-xxxxxxxxxx!",
            action: "decrypt",
            viewer_interface: "video",
            output_kind: "frames",
            expires_at: 1_900_000_000,
            release_receipt_hash: [7u8; 32],
            decrypt_session_pub: b"published-session-pubkey-bytes",
            nonce: b"replay-nonce-1",
            node_set_id: None,
        }
    }

    /// The multi-segment binding is strictly ADDITIVE: `to_aad()` equals
    /// `to_aad_with_segments(None)` byte-for-byte (single-segment opens are untouched), while a
    /// present digest list strictly extends the AAD and is sensitive to segment order/content —
    /// so a CEK sealed for an ordered segment set cannot open under a reordered or altered set.
    #[test]
    fn transcript_segment_binding_is_additive_and_ordered() {
        let t = sample_transcript();
        assert_eq!(t.to_aad(), t.to_aad_with_segments(None), "absent digests == plain to_aad");

        let seg_a = b"segment-zero-bytes".as_slice();
        let seg_b = b"segment-one-bytes-longer".as_slice();
        let d_ab = crate::segment_digests(&[seg_a, seg_b]);
        let bound = t.to_aad_with_segments(Some(&d_ab));
        assert_ne!(t.to_aad(), bound, "binding a segment list extends the AAD");
        assert!(bound.starts_with(&t.to_aad()), "the binding strictly EXTENDS the single-segment AAD");

        // Reordering the segments changes the binding (order is welded in).
        let d_ba = crate::segment_digests(&[seg_b, seg_a]);
        assert_ne!(bound, t.to_aad_with_segments(Some(&d_ba)), "segment ORDER is bound");
        // Substituting a segment changes the binding.
        let d_ax = crate::segment_digests(&[seg_a, b"tampered".as_slice()]);
        assert_ne!(bound, t.to_aad_with_segments(Some(&d_ax)), "segment CONTENT is bound");
    }

    /// The AV variant-set binding is strictly ADDITIVE (chunk 4): a `None` commitment leaves the
    /// AAD byte-identical to `to_aad_with_bindings`, while a present commitment strictly extends it
    /// AFTER the rights binding and is sensitive to the set — so a CEK sealed for one published
    /// variant set cannot open under a swapped/forged manifest.
    #[test]
    fn transcript_variant_set_binding_is_additive_and_bound() {
        let t = sample_transcript();
        let segs = crate::segment_digests(&[b"a".as_slice(), b"b".as_slice()]);
        let rights = [7u8; 32];
        let base = t.to_aad_with_bindings(Some(&segs), Some(&rights));
        assert_eq!(
            base,
            t.to_aad_with_all_bindings(Some(&segs), Some(&rights), None),
            "absent variant commitment == to_aad_with_bindings (goldens replay unchanged)"
        );
        let vsc = [9u8; 32];
        let bound = t.to_aad_with_all_bindings(Some(&segs), Some(&rights), Some(&vsc));
        assert_ne!(base, bound, "binding the variant set extends the AAD");
        assert!(bound.starts_with(&base), "the variant set binding strictly EXTENDS the rights AAD");
        let vsc2 = [10u8; 32];
        assert_ne!(
            bound,
            t.to_aad_with_all_bindings(Some(&segs), Some(&rights), Some(&vsc2)),
            "a different published variant set changes the binding (manifest swap fails closed)"
        );
    }

    /// The encoder is deterministic and self-describing (domain label first), so the
    /// authority and the decrypt boundary derive byte-identical AADs from equal fields.
    #[test]
    fn transcript_aad_is_deterministic_and_labelled() {
        let aad = sample_transcript().to_aad();
        assert_eq!(aad, sample_transcript().to_aad(), "equal fields -> equal AAD");
        // First length-prefixed field is the domain label.
        let label = crate::transcript::DECRYPT_TRANSCRIPT_LABEL;
        assert_eq!(&aad[..4], &(label.len() as u32).to_be_bytes());
        assert_eq!(&aad[4..4 + label.len()], label);
    }

    /// Any field change yields a different AAD — the binding is total, so a CEK sealed
    /// for one transcript cannot open under another (replay / field-swap defense).
    #[test]
    fn transcript_aad_changes_with_every_field() {
        let base = sample_transcript().to_aad();
        let mut t = sample_transcript();
        t.session_id = "sess-2";
        assert_ne!(base, t.to_aad(), "session change must change the AAD");
        let mut t = sample_transcript();
        t.nonce = b"replay-nonce-2";
        assert_ne!(base, t.to_aad(), "nonce change must change the AAD");
        let mut t = sample_transcript();
        t.expires_at += 1;
        assert_ne!(base, t.to_aad(), "expiry change must change the AAD");
    }

    /// The 2-of-2 node-set identity is welded into the transcript AAD when present — a release
    /// is cryptographically bound to the EXACT set of secret-holders — while the single-node
    /// (`None`) encoding stays byte-identical to the pre-threshold transcript.
    #[test]
    fn transcript_aad_binds_the_node_set_and_keeps_single_node_byte_identical() {
        let single = sample_transcript().to_aad();

        let id_a = crate::threshold_node_set_id(2, b"vk-node-a", b"vk-node-b");
        let mut t = sample_transcript();
        t.node_set_id = Some(&id_a);
        let with_set = t.to_aad();

        // A threshold transcript is a STRICT extension of the single-node one: same prefix
        // (no existing AAD changed), then the length-prefixed node-set id — so the two can
        // never be equal and a single-node seal can never open as a threshold one.
        assert_ne!(single, with_set, "binding a node-set must change the AAD");
        assert_eq!(&with_set[..single.len()], single.as_slice(), "the single-node encoding is unchanged");
        assert_eq!(with_set.len(), single.len() + 4 + id_a.len(), "exactly one length-prefixed field appended");

        // A DIFFERENT node-set (one node swapped) yields a DIFFERENT AAD — the swapped-node
        // release fails the AEAD open at the boundary, not just at descriptor parse.
        let id_b = crate::threshold_node_set_id(2, b"vk-node-a", b"vk-node-ROGUE");
        let mut t = sample_transcript();
        t.node_set_id = Some(&id_b);
        assert_ne!(with_set, t.to_aad(), "a swapped node-set must change the AAD");
    }

    /// The dKMS-node identity handshake: a node's attestation over a challenge verifies under its
    /// PINNED verifying key, and any forgery/tamper fails — so a client can pin + verify the node
    /// before delegating recovery.
    #[test]
    fn dkms_hello_attestation_round_trips_and_rejects_forgery() {
        let (signer, vk) = crate::seal::mldsa_seal_keypair([0x42u8; 32]);
        let verifier = MlDsa65Verifier::from_encoded(&vk).unwrap();
        let challenge = [0x9au8; 32];
        let attestation = crate::attest_challenge(&signer, &challenge);
        assert!(
            crate::verify_attestation(&verifier, &challenge, &attestation),
            "the genuine node's attestation verifies under its pinned vk"
        );

        // A different (impersonating) node's key does NOT verify under the pinned vk.
        let (other_signer, _other_vk) = crate::seal::mldsa_seal_keypair([0x43u8; 32]);
        let forged = crate::attest_challenge(&other_signer, &challenge);
        assert!(
            !crate::verify_attestation(&verifier, &challenge, &forged),
            "an impersonating node's attestation must be rejected under the pinned vk"
        );

        // A tampered challenge (replay against a different nonce) fails.
        let mut other_challenge = challenge;
        other_challenge[0] ^= 1;
        assert!(
            !crate::verify_attestation(&verifier, &other_challenge, &attestation),
            "an attestation over a different challenge must not verify"
        );

        // A malformed signature fails closed (no panic).
        assert!(!crate::verify_attestation(&verifier, &challenge, b"not-a-signature"));
    }

    /// The hello attestation is domain-separated from CEK seals — a seal signature can never be
    /// replayed as a node-identity attestation (different signed prefix).
    #[test]
    fn dkms_hello_is_domain_separated_from_seals() {
        let (signer, vk) = crate::seal::mldsa_seal_keypair([7u8; 32]);
        let verifier = MlDsa65Verifier::from_encoded(&vk).unwrap();
        let challenge = [1u8; 32];
        // A bare signature over the challenge WITHOUT the hello domain must not verify as an attestation.
        use crate::seal::CekSealSigner as _;
        let bare = signer.sign(&challenge);
        assert!(!crate::verify_attestation(&verifier, &challenge, &bare));
    }

    /// A node session token verifies under the node's own vk for its `(challenge, expires_at)`, and
    /// any tamper (challenge / expiry / forged key / malformed sig) fails — so the node can require a
    /// live, unforgeable token on every recover.
    #[test]
    fn dkms_session_token_round_trips_and_rejects_tamper() {
        let (signer, vk) = crate::seal::mldsa_seal_keypair([0x5au8; 32]);
        let verifier = MlDsa65Verifier::from_encoded(&vk).unwrap();
        let challenge = [0x33u8; 32];
        let caller_pub = [0x77u8; 48];
        let expires_at = 1_000_000u64;
        let token = crate::sign_session_token(&signer, &challenge, &caller_pub, expires_at);
        assert!(crate::verify_session_token(&verifier, &challenge, &caller_pub, expires_at, &token));

        // Tampering the expiry (e.g. extending the window) invalidates the signature.
        assert!(!crate::verify_session_token(&verifier, &challenge, &caller_pub, expires_at + 1, &token));

        // Tampering the challenge invalidates the signature.
        let mut other = challenge;
        other[0] ^= 1;
        assert!(!crate::verify_session_token(&verifier, &other, &caller_pub, expires_at, &token));

        // Tampering the bound caller pubkey invalidates the signature (the token is caller-bound).
        let mut other_pub = caller_pub;
        other_pub[0] ^= 1;
        assert!(!crate::verify_session_token(&verifier, &challenge, &other_pub, expires_at, &token));

        // A token forged by a different (impersonator) key does not verify under this vk.
        let (impostor, _ivk) = crate::seal::mldsa_seal_keypair([0x5bu8; 32]);
        let forged = crate::sign_session_token(&impostor, &challenge, &caller_pub, expires_at);
        assert!(!crate::verify_session_token(&verifier, &challenge, &caller_pub, expires_at, &forged));

        // A malformed signature fails closed (no panic).
        assert!(!crate::verify_session_token(&verifier, &challenge, &caller_pub, expires_at, b"nope"));
    }

    /// The session token is domain-separated from the hello attestation — a hello attestation over
    /// the same challenge can never be replayed as a session token (different signed prefix/shape).
    #[test]
    fn dkms_session_token_is_domain_separated_from_hello() {
        let (signer, vk) = crate::seal::mldsa_seal_keypair([9u8; 32]);
        let verifier = MlDsa65Verifier::from_encoded(&vk).unwrap();
        let challenge = [2u8; 32];
        let caller_pub = [3u8; 48];
        let expires_at = 42u64;
        // A hello attestation must NOT verify as a session token over the same challenge.
        let hello = crate::attest_challenge(&signer, &challenge);
        assert!(!crate::verify_session_token(&verifier, &challenge, &caller_pub, expires_at, &hello));
        // …and a session token must NOT verify as a hello attestation.
        let token = crate::sign_session_token(&signer, &challenge, &caller_pub, expires_at);
        assert!(!crate::verify_attestation(&verifier, &challenge, &token));
    }

    /// The recover possession proof: a signature under the caller's ephemeral key over the session
    /// challenge + the recover binding verifies under that key — and a proof from a DIFFERENT key, a
    /// tampered binding, or a malformed signature all fail. So a captured bearer token replayed by a
    /// caller WITHOUT the matching private key cannot drive recovery.
    #[test]
    fn dkms_recover_proof_round_trips_and_rejects_wrong_key_or_tamper() {
        let (caller, caller_vk) = crate::seal::mldsa_seal_keypair([0x61u8; 32]);
        let caller_verifier = MlDsa65Verifier::from_encoded(&caller_vk).unwrap();
        let challenge = [0x12u8; 32];
        let (content, kid, sess_pub) = (b"bafContent".as_slice(), b"c5c5".as_slice(), b"sessionpub".as_slice());
        let seq = 1u64;
        let aad = b"re-seal-transcript-aad".as_slice();
        let proof = crate::sign_recover_proof(&caller, &challenge, content, kid, sess_pub, seq, aad);
        assert!(crate::verify_recover_proof(&caller_verifier, &challenge, content, kid, sess_pub, seq, aad, &proof));

        // A proof from a DIFFERENT key (a captured-token replayer without the private key) fails.
        let (other, _ovk) = crate::seal::mldsa_seal_keypair([0x62u8; 32]);
        let wrong = crate::sign_recover_proof(&other, &challenge, content, kid, sess_pub, seq, aad);
        assert!(!crate::verify_recover_proof(&caller_verifier, &challenge, content, kid, sess_pub, seq, aad, &wrong));

        // A tampered binding (different content / kid / session pub / challenge / freshness seq) fails.
        assert!(!crate::verify_recover_proof(&caller_verifier, &challenge, b"bafOTHER", kid, sess_pub, seq, aad, &proof));
        assert!(!crate::verify_recover_proof(&caller_verifier, &challenge, content, b"ffff", sess_pub, seq, aad, &proof));
        assert!(!crate::verify_recover_proof(&caller_verifier, &challenge, content, kid, b"otherpub", seq, aad, &proof));
        assert!(!crate::verify_recover_proof(&caller_verifier, b"otherchal", content, kid, sess_pub, seq, aad, &proof));
        // A SWAPPED freshness counter invalidates the proof (the seq is authenticated, not free to alter).
        assert!(!crate::verify_recover_proof(&caller_verifier, &challenge, content, kid, sess_pub, seq + 1, aad, &proof));
        // A TAMPERED re-seal AAD (v2 binding) invalidates the proof — the node will not seal under an
        // AAD the caller did not prove possession over. This is the re-seal-AAD invariant, enforced.
        assert!(!crate::verify_recover_proof(&caller_verifier, &challenge, content, kid, sess_pub, seq, b"tampered-aad", &proof));

        // The possession proof is domain-separated from the session token (different domain prefix).
        assert!(!crate::verify_session_token(&caller_verifier, &challenge, content, 0, &proof));

        // A malformed signature fails closed.
        assert!(!crate::verify_recover_proof(&caller_verifier, &challenge, content, kid, sess_pub, seq, aad, b"nope"));
    }

    /// The socket framing round-trips messages, recovers exact boundaries from a concatenated
    /// stream, signals a clean EOF at a boundary, and fails closed on a torn frame.
    #[test]
    fn frame_round_trips_and_fails_closed_on_torn_or_oversized() {
        use crate::frame::{read_frame, write_frame, MAX_FRAME_BYTES};
        // Two frames written back-to-back are read back as two distinct messages, then a clean EOF.
        let mut buf = Vec::new();
        write_frame(&mut buf, b"first").unwrap();
        write_frame(&mut buf, b"{\"op\":\"hello\"}").unwrap();
        let mut cur = std::io::Cursor::new(buf);
        assert_eq!(read_frame(&mut cur).unwrap().as_deref(), Some(b"first".as_slice()));
        assert_eq!(read_frame(&mut cur).unwrap().as_deref(), Some(b"{\"op\":\"hello\"}".as_slice()));
        assert!(read_frame(&mut cur).unwrap().is_none(), "clean EOF at a frame boundary -> None");

        // A header promising more bytes than follow is a TORN frame -> error (never a partial parse).
        let mut torn = Vec::new();
        torn.extend_from_slice(&7u32.to_be_bytes());
        torn.extend_from_slice(b"abc");
        assert!(read_frame(&mut std::io::Cursor::new(torn)).is_err());

        // An oversized length header is refused before allocating.
        let mut huge = Vec::new();
        huge.extend_from_slice(&(MAX_FRAME_BYTES + 1).to_be_bytes());
        assert!(read_frame(&mut std::io::Cursor::new(huge)).is_err());

        // A zero-length frame is refused.
        let zero = 0u32.to_be_bytes().to_vec();
        assert!(read_frame(&mut std::io::Cursor::new(zero)).is_err());

        // Writing an oversized payload is refused fail-closed.
        let mut sink = Vec::new();
        let too_big = vec![0u8; (MAX_FRAME_BYTES + 1) as usize];
        assert!(write_frame(&mut sink, &too_big).is_err());
    }

    /// Channel-frame padding hides the exact plaintext size behind coarse buckets, always round-trips
    /// (even when the plaintext ends in 0x00), and fails closed on a malformed pad.
    #[test]
    fn channel_pad_buckets_round_trip_and_fail_closed() {
        use crate::channel_pad::{pad, unpad};
        // Distinct small sizes collapse onto the SAME bucket length (the metadata-hiding property).
        let a = pad(b"{\"op\":\"status\"}");
        let b = pad(b"{\"op\":\"hello\",\"x\":1}");
        assert_eq!(a.len(), 256, "small control messages land in the 256B bucket");
        assert_eq!(a.len(), b.len(), "two differently-sized small messages share a bucket");

        // Round-trip across a range of sizes, including a payload that ends in 0x00 (the case the
        // mandatory 0x80 marker exists to disambiguate) and an exact-power-of-two boundary.
        for plaintext in [
            vec![],
            b"a".to_vec(),
            vec![0u8; 255],         // +marker crosses into the 512 bucket
            vec![7u8; 256],
            {
                let mut v = b"trailing-zeros".to_vec();
                v.extend_from_slice(&[0u8; 5]);
                v
            },
            vec![0x42u8; 200_000],  // above TOP_BUCKET -> marker-only, still round-trips
        ] {
            let padded = pad(&plaintext);
            assert!(padded.len() > plaintext.len(), "padding always grows by >= the marker");
            assert_eq!(unpad(&padded).as_deref(), Some(plaintext.as_slice()));
        }

        // A pad with no 0x80 marker (all zeros) is malformed -> None (fail closed).
        assert_eq!(unpad(&[0u8; 16]), None);
        assert_eq!(unpad(&[]), None);
    }

    /// Receive-side tolerance: `unpad_incoming` accepts BOTH wire forms so a padded and an
    /// un-padded peer interoperate regardless of which side ships padding first. This is the
    /// property that keeps the channel backward-compatible with the deployed (un-padded) quorum
    /// nodes — without it, enabling padding on the client breaks every open against an old node.
    #[test]
    fn channel_pad_incoming_accepts_padded_and_unpadded_peers() {
        use crate::channel_pad::{pad, unpad_incoming};
        // A raw (un-padded) JSON frame from a legacy node round-trips untouched.
        let raw = br#"{"status":"released","share_b64":"AA=="}"#.to_vec();
        assert_eq!(unpad_incoming(&raw), raw, "un-padded peer frame passes through unchanged");
        // A padded frame from a padding-aware node is stripped back to the exact plaintext.
        assert_eq!(unpad_incoming(&pad(&raw)), raw, "padded peer frame is stripped to plaintext");
        // Even a binary plaintext that itself ends in 0x80 strips correctly (marker is the last
        // non-zero byte), so the tolerance is unambiguous beyond JSON too.
        let ends_in_marker = vec![1u8, 2, 0x80];
        assert_eq!(unpad_incoming(&pad(&ends_in_marker)), ends_in_marker);
    }

    /// The 2-of-2 XOR share-split round-trips, hides the CEK in each share alone, and
    /// fails closed on a length mismatch (a wrong/forged share can never yield a key).
    #[test]
    fn cek_xor_split_round_trips_and_fails_closed() {
        let cek: Vec<u8> = (0u8..16).collect();
        let mask: Vec<u8> = (0u8..16).map(|b| b ^ 0xA5).collect();

        let (share1, share2) = crate::split_cek_xor(&cek, &mask).expect("split");
        // No single share equals (or reveals) the CEK.
        assert_ne!(share1.as_slice(), cek.as_slice(), "share1 alone must not be the CEK");
        assert_ne!(share2.as_slice(), cek.as_slice(), "share2 alone must not be the CEK");
        assert_eq!(share1.as_slice(), mask.as_slice(), "share1 is the random mask");

        // Combining the two shares reconstructs the CEK exactly.
        let recovered = crate::combine_cek_xor(&share1, &share2).expect("combine");
        assert_eq!(recovered.as_slice(), cek.as_slice(), "share1 ^ share2 == CEK");

        // Order does not matter (XOR is commutative).
        let recovered_swapped = crate::combine_cek_xor(&share2, &share1).expect("combine swapped");
        assert_eq!(recovered_swapped.as_slice(), cek.as_slice());

        // ONE share alone is useless: combining a share with itself (the single-node
        // attacker who only ever sees one share) yields zeros, never the CEK.
        let single = crate::combine_cek_xor(&share1, &share1).expect("combine self");
        assert!(single.iter().all(|&b| b == 0));
        assert_ne!(single.as_slice(), cek.as_slice());

        // A wrong-length mask or share fails closed (no silent truncation).
        assert!(crate::split_cek_xor(&cek, &mask[..15]).is_err());
        assert!(crate::split_cek_xor(&[], &[]).is_err());
        assert!(crate::combine_cek_xor(&share1, &share2[..15]).is_err());
        assert!(crate::combine_cek_xor(&[], &[]).is_err());
    }

    /// Shamir 2-of-3 over GF(256): ANY two of the three shares reconstruct the CEK exactly;
    /// a single share is information-theoretically useless; malformed quorums fail closed.
    #[test]
    fn cek_shamir_2of3_any_pair_reconstructs_and_fails_closed() {
        let cek: Vec<u8> = (0u8..32).collect();
        let coeff: Vec<u8> = (0u8..32).map(|b| b.wrapping_mul(7) ^ 0x3C).collect();
        let shares = crate::split_cek_shamir2(&cek, &coeff).expect("split");

        // ALL THREE pairs reconstruct the identical CEK — this is what survives a dead node.
        for (xa, xb) in [(1u8, 2u8), (1, 3), (2, 3)] {
            let a = &shares[(xa - 1) as usize];
            let b = &shares[(xb - 1) as usize];
            let rec = crate::combine_cek_shamir2(xa, a, xb, b).expect("combine");
            assert_eq!(rec.as_slice(), cek.as_slice(), "pair ({xa},{xb}) must reconstruct the CEK");
            // Order of the pair does not matter.
            let rec_swapped = crate::combine_cek_shamir2(xb, b, xa, a).expect("combine swapped");
            assert_eq!(rec_swapped.as_slice(), cek.as_slice());
        }

        // No single share is the CEK, and a sub-quorum is structurally refused: combining a
        // share with ITSELF (duplicate x — one node's view twice) is not a quorum.
        for (i, share) in shares.iter().enumerate() {
            assert_ne!(share.as_slice(), cek.as_slice(), "share {} alone must not be the CEK", i + 1);
            assert!(crate::combine_cek_shamir2((i + 1) as u8, share, (i + 1) as u8, share).is_err());
        }
        // INFORMATION-THEORETIC uselessness: two different coefficient vectors produce splits
        // where the SAME x-1 share bytes are consistent with DIFFERENT CEKs — a single share
        // pins nothing. (Pick coeff' so share1 collides: cek' ⊕ k'·1 == cek ⊕ k·1.)
        let cek2: Vec<u8> = cek.iter().map(|&b| b ^ 0xFF).collect();
        let coeff2: Vec<u8> = cek2
            .iter()
            .zip(shares[0].iter())
            .map(|(&c2, &s1)| c2 ^ s1) // k'·1 = k' = cek'[j] ⊕ share1[j]
            .collect();
        let shares2 = crate::split_cek_shamir2(&cek2, &coeff2).expect("split 2");
        assert_eq!(shares2[0], shares[0], "the same share-1 bytes serve BOTH CEKs");
        assert_ne!(cek, cek2);

        // MIXED-SPLIT shares (x=2 of split 1 + x=3 of split 2 — two shares that never came
        // from one split) reconstruct GARBAGE, never either CEK.
        let mixed = crate::combine_cek_shamir2(2, &shares[1], 3, &shares2[2]).expect("mixed combines");
        assert_ne!(mixed.as_slice(), cek.as_slice());
        assert_ne!(mixed.as_slice(), cek2.as_slice());

        // Fail-closed shapes: zero x (x=0 IS the secret), length mismatch, empty.
        assert!(crate::combine_cek_shamir2(0, &shares[0], 2, &shares[1]).is_err());
        assert!(crate::combine_cek_shamir2(1, &shares[0], 0, &shares[1]).is_err());
        assert!(crate::combine_cek_shamir2(1, &shares[0], 2, &shares[1][..31]).is_err());
        assert!(crate::combine_cek_shamir2(1, &[], 2, &[]).is_err());
        assert!(crate::split_cek_shamir2(&cek, &coeff[..31]).is_err());
        assert!(crate::split_cek_shamir2(&[], &[]).is_err());

        // The XOR-rotation refresh delta carries over: refresh every share with q(x_i) for a
        // random q with q(0)=0 (q(x) = c·x) and the SAME CEK still reconstructs from the
        // refreshed shares, while an old+new mix is garbage (proactive refresh invalidates).
        let c = 0x5Au8;
        let refreshed: Vec<Vec<u8>> = shares
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let qx = crate::gf256_mul(c, (i + 1) as u8);
                s.iter().map(|&b| b ^ qx).collect()
            })
            .collect();
        let rec = crate::combine_cek_shamir2(1, &refreshed[0], 3, &refreshed[2]).expect("refreshed");
        assert_eq!(rec.as_slice(), cek.as_slice(), "a q(0)=0 refresh preserves the CEK");
        let mixed_gen = crate::combine_cek_shamir2(1, &shares[0], 3, &refreshed[2]).expect("old+new");
        assert_ne!(mixed_gen.as_slice(), cek.as_slice(), "old+refreshed shares must NOT reconstruct");
    }

    /// PRE-AUDIT FINDING #1 — CEK reconstruction integrity. The 3-share checked combine
    /// reconstructs the CEK when all three shares are consistent, but FAILS CLOSED the moment
    /// ANY one of the three carries a well-formed, correctly-indexed, but WRONG value (a single
    /// Byzantine quorum node). And the published commitment catches a wrong CEK independently.
    #[test]
    fn cek_quorum_checked_detects_a_byzantine_share_and_commitment_binds() {
        let cek: Vec<u8> = (0u8..16).map(|b| b.wrapping_mul(11) ^ 0x5A).collect();
        let coeff: Vec<u8> = (0u8..16).map(|b| b.wrapping_mul(7) ^ 0x3C).collect();
        let shares = crate::split_cek_shamir2(&cek, &coeff).expect("split");
        let pts = |s: &[Vec<u8>]| -> Vec<(u8, Vec<u8>)> {
            s.iter().enumerate().map(|(i, v)| ((i + 1) as u8, v.clone())).collect()
        };

        // All three honest shares are consistent ⇒ the checked combine reconstructs the CEK.
        let honest = pts(&shares);
        let refs: Vec<(u8, &[u8])> = honest.iter().map(|(x, v)| (*x, v.as_slice())).collect();
        let rec = crate::combine_cek_shamir2_checked(&refs).expect("3 honest shares reconstruct");
        assert_eq!(rec.as_slice(), cek.as_slice());

        // A single wrong-valued share (well-formed, correctly indexed) is DETECTED for every
        // position it could occupy — the open fails closed, never yields a silently-wrong key.
        for bad in 0..3usize {
            let mut tampered = shares.clone();
            tampered[bad][0] ^= 0x01; // flip one byte: still well-formed, wrong value
            let bad_pts = pts(&tampered);
            let bad_refs: Vec<(u8, &[u8])> = bad_pts.iter().map(|(x, v)| (*x, v.as_slice())).collect();
            assert!(
                crate::combine_cek_shamir2_checked(&bad_refs).is_err(),
                "a wrong-valued share at position {bad} must fail the checked combine closed"
            );
        }

        // Fewer than three shares: the checked combine REFUSES (no false sense of detection).
        let two: Vec<(u8, &[u8])> = refs[..2].to_vec();
        assert!(crate::combine_cek_shamir2_checked(&two).is_err());

        // The published commitment binds the CEK to the node-set and rejects a wrong CEK.
        let node_set_id = crate::threshold_node_set_id_n(2, &[&[0xA1u8; 40][..], &[0xB2u8; 40][..], &[0xC3u8; 40][..]]);
        let commitment = crate::cek_commitment(&node_set_id, &cek);
        assert!(crate::verify_cek_commitment(&node_set_id, &cek, &commitment));
        let mut wrong = cek.clone();
        wrong[0] ^= 0x01;
        assert!(!crate::verify_cek_commitment(&node_set_id, &wrong, &commitment), "commitment rejects a wrong CEK");
        // A commitment is bound to its node-set: another quorum's commitment does not verify.
        let other_set = crate::threshold_node_set_id_n(2, &[&[0x11u8; 40][..], &[0x22u8; 40][..], &[0x33u8; 40][..]]);
        assert!(!crate::verify_cek_commitment(&other_set, &cek, &commitment), "commitment is node-set bound");
    }

    /// GOLDEN VECTOR: the Shamir split/combine is pinned byte-for-byte (a refactor or a GF
    /// arithmetic change that alters the wire shares is caught here, like the envelope goldens).
    #[test]
    fn cek_shamir_2of3_golden_vector() {
        let cek = [0xC5u8, 0x01, 0x7F, 0xFF, 0x00, 0xAB, 0x10, 0x42];
        let coeff = [0x02u8, 0x80, 0xFF, 0x1B, 0x01, 0x00, 0xA5, 0x33];
        let shares = crate::split_cek_shamir2(&cek, &coeff).expect("split");
        // p(x) = cek ⊕ coeff·x over GF(2^8)/0x11B, evaluated at x = 1, 2, 3.
        assert_eq!(shares[0], vec![0xC7, 0x81, 0x80, 0xE4, 0x01, 0xAB, 0xB5, 0x71]);
        assert_eq!(shares[1], vec![0xC1, 0x1A, 0x9A, 0xC9, 0x02, 0xAB, 0x41, 0x24]);
        assert_eq!(shares[2], vec![0xC3, 0x9A, 0x65, 0xD2, 0x03, 0xAB, 0xE4, 0x17]);
        for (xa, xb) in [(1u8, 2u8), (1, 3), (2, 3)] {
            let rec = crate::combine_cek_shamir2(
                xa,
                &shares[(xa - 1) as usize],
                xb,
                &shares[(xb - 1) as usize],
            )
            .expect("golden combine");
            assert_eq!(rec.as_slice(), cek.as_slice());
        }
    }

    /// GENERAL t-of-n (`split_cek_shamir`): a 3-of-5 split where ANY 3 shares reconstruct the
    /// CEK via the general Lagrange combine, any 2 do NOT, and the 1-coeff/n=3 case is
    /// byte-identical to the pinned `split_cek_shamir2` (so the generalization cannot drift).
    #[test]
    fn cek_shamir_general_t_of_n_reconstructs_and_fails_closed() {
        let cek: Vec<u8> = (0u8..24).map(|b| b.wrapping_mul(5) ^ 0x11).collect();
        // t = 3 ⇒ a degree-2 polynomial ⇒ 2 coefficient vectors; n = 5 nodes.
        let c1: Vec<u8> = (0u8..24).map(|b| b.wrapping_mul(7) ^ 0x3C).collect();
        let c2: Vec<u8> = (0u8..24).map(|b| b.wrapping_mul(13) ^ 0x5A).collect();
        let shares = crate::split_cek_shamir(&cek, &[&c1, &c2], 5).expect("split 3-of-5");
        assert_eq!(shares.len(), 5);

        // EVERY size-3 subset of the 5 coordinates reconstructs the exact CEK.
        let coords = [1u8, 2, 3, 4, 5];
        for i in 0..5 {
            for j in (i + 1)..5 {
                for k in (j + 1)..5 {
                    let pts: Vec<(u8, &[u8])> = vec![
                        (coords[i], shares[i].as_slice()),
                        (coords[j], shares[j].as_slice()),
                        (coords[k], shares[k].as_slice()),
                    ];
                    let rec = crate::lagrange_combine_at_zero(&pts).expect("combine triple");
                    assert_eq!(rec.as_slice(), cek.as_slice(), "subset ({},{},{}) must reconstruct", coords[i], coords[j], coords[k]);
                }
            }
        }

        // BELOW THRESHOLD: 2 points cannot reconstruct a degree-2 secret — the result differs.
        let two: Vec<(u8, &[u8])> = vec![(1, shares[0].as_slice()), (2, shares[1].as_slice())];
        let under = crate::lagrange_combine_at_zero(&two).expect("combines but is wrong");
        assert_ne!(under.as_slice(), cek.as_slice(), "t-1 shares must NOT reconstruct the secret");

        // CONSISTENCY: the general split with one coeff + n=3 equals the pinned 2-of-3 split.
        let coeff: Vec<u8> = (0u8..24).map(|b| b.wrapping_mul(7) ^ 0x3C).collect();
        let general = crate::split_cek_shamir(&cek, &[&coeff], 3).expect("general 2-of-3");
        let special = crate::split_cek_shamir2(&cek, &coeff).expect("special 2-of-3");
        assert_eq!(general, special.to_vec(), "the generalization must match split_cek_shamir2 byte-for-byte");

        // FAIL-CLOSED shapes: empty cek, n=0, coeff length mismatch.
        assert!(crate::split_cek_shamir(&[], &[&c1], 5).is_err());
        assert!(crate::split_cek_shamir(&cek, &[&c1, &c2], 0).is_err());
        assert!(crate::split_cek_shamir(&cek, &[&c1[..23]], 5).is_err());
    }

    /// The indexed-share escrow encoding (`x ‖ share`) round-trips and fails closed on a
    /// zero x or an empty payload — the x rides INSIDE the sealed envelope, authenticated.
    #[test]
    fn indexed_share_round_trips_and_fails_closed() {
        let share = vec![0xAAu8; 16];
        let payload = crate::indexed_share(3, &share);
        assert_eq!(payload.len(), 17);
        let (x, parsed) = crate::parse_indexed_share(&payload).expect("parse");
        assert_eq!(x, 3);
        assert_eq!(parsed, share.as_slice());
        // x = 0 is never a valid Shamir coordinate (x=0 is the secret itself).
        assert!(crate::parse_indexed_share(&crate::indexed_share(0, &share)).is_none());
        // Empty / index-only payloads are refused.
        assert!(crate::parse_indexed_share(&[]).is_none());
        assert!(crate::parse_indexed_share(&[2]).is_none());
    }

    /// PROACTIVE REFRESH of the 2-of-3 quorum (Day 117–120): rotating ALL THREE shares with
    /// per-node deltas `q(x_i)` derived from ONE fresh `refresh_coeff` keeps the CEK invariant
    /// (any TWO refreshed shares still reconstruct it — the quorum survives the refresh) WHILE
    /// killing old material (an OLD share next to a REFRESHED share is garbage). Also proves the
    /// delta is COORDINATE-BOUND (the x prefix is preserved; a wrong-coordinate delta corrupts the
    /// pair) and fails closed on bad inputs.
    #[test]
    fn shamir_refresh_keeps_cek_invariant_and_kills_old_material() {
        let cek = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let coeff = [0x9Au8, 0xBC, 0xDE, 0xF0, 0x12, 0x34, 0x56, 0x78];
        let shares = crate::split_cek_shamir2(&cek, &coeff).expect("split");
        // Escrowed payloads are the INDEXED shares the nodes hold + rotate.
        let indexed: Vec<Vec<u8>> =
            (0..3).map(|i| crate::indexed_share((i + 1) as u8, &shares[i])).collect();

        // ONE fresh refresh coefficient → per-node coordinate-bound deltas q(x_i).
        let refresh = [0x0Fu8, 0x1E, 0x2D, 0x3C, 0x4B, 0x5A, 0x69, 0x78];
        let rotate = |payload: &[u8], x: u8| -> Vec<u8> {
            let delta = crate::shamir_refresh_delta(&refresh, x).expect("delta");
            assert_eq!(delta.len(), payload.len(), "delta must match the indexed-share length");
            assert_eq!(delta[0], 0, "the index prefix byte must be untouched");
            // The node's UNCHANGED blind XOR.
            payload.iter().zip(delta.iter()).map(|(a, b)| a ^ b).collect()
        };
        let refreshed: Vec<Vec<u8>> =
            (0..3).map(|i| rotate(&indexed[i], (i + 1) as u8)).collect();

        // The x prefix survived on every refreshed share, and the body changed.
        for i in 0..3 {
            let (x, body) = crate::parse_indexed_share(&refreshed[i]).expect("refreshed parse");
            assert_eq!(x, (i + 1) as u8, "the coordinate is preserved across rotation");
            assert_ne!(body, shares[i].as_slice(), "the share body was refreshed");
        }

        // INVARIANT: any TWO refreshed shares reconstruct the EXACT original CEK.
        let combine = |a: &[u8], b: &[u8]| -> Vec<u8> {
            let (xa, sa) = crate::parse_indexed_share(a).unwrap();
            let (xb, sb) = crate::parse_indexed_share(b).unwrap();
            crate::combine_cek_shamir2(xa, sa, xb, sb).expect("combine").to_vec()
        };
        assert_eq!(combine(&refreshed[0], &refreshed[1]), cek, "A'+B' reconstructs the CEK");
        assert_eq!(combine(&refreshed[0], &refreshed[2]), cek, "A'+C' reconstructs the CEK");
        assert_eq!(combine(&refreshed[1], &refreshed[2]), cek, "B'+C' reconstructs the CEK");

        // OLD MATERIAL DEAD: an OLD share next to a REFRESHED share no longer interpolates the CEK.
        assert_ne!(combine(&indexed[0], &refreshed[1]), cek, "old A + refreshed B is garbage");
        assert_ne!(combine(&indexed[1], &refreshed[2]), cek, "old B + refreshed C is garbage");

        // COORDINATE-BOUND: rotating node A (x=1) with node B's delta (q at x=2) puts A' on no
        // shared polynomial — the pair touching it reconstructs garbage, while B'+C' still works.
        let mis_a = {
            let wrong = crate::shamir_refresh_delta(&refresh, 2).expect("delta@2");
            indexed[0].iter().zip(wrong.iter()).map(|(a, b)| a ^ b).collect::<Vec<u8>>()
        };
        assert_ne!(combine(&mis_a, &refreshed[1]), cek, "a coordinate-mismatched delta breaks the pair");
        assert_eq!(combine(&refreshed[1], &refreshed[2]), cek, "the correctly-refreshed pair is unaffected");

        // Fail-closed surface.
        assert!(crate::shamir_refresh_delta(&refresh, 0).is_err(), "x=0 is the secret, never a node");
        assert!(crate::shamir_refresh_delta(&[], 1).is_err(), "empty refresh_coeff is refused");
    }

    /// Quorum RECONFIGURATION (Day 121–125): a live 2-of-3 set is RE-SHARED into a 3-of-5 set that
    /// reconstructs the SAME CEK at a NEW threshold + NEW membership, the CEK never reassembling.
    /// Each OLD quorum member sub-shares its share under a fresh degree-2 polynomial (k=3); each NEW
    /// node combines the sub-shares from the old quorum into its new share; ANY THREE new shares
    /// reconstruct the CEK, any TWO do not, and OLD material is dead against the new set.
    #[test]
    fn reshare_2of3_to_3of5_keeps_cek_and_lifts_the_threshold() {
        let cek = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let coeff = [0x9Au8, 0xBC, 0xDE, 0xF0, 0x12, 0x34, 0x56, 0x78];
        // OLD 2-of-3 set: shares p(1), p(2), p(3) on a degree-1 polynomial.
        let old = crate::split_cek_shamir2(&cek, &coeff).expect("old split");
        // Sanity: any two old shares already reconstruct via the general combine.
        assert_eq!(
            crate::lagrange_combine_at_zero(&[(1, &old[0]), (2, &old[1])]).expect("old combine").to_vec(),
            cek,
            "the general Lagrange combine reconstructs the old 2-of-3 secret"
        );

        // RECONFIGURE to 3-of-5 using the OLD quorum {x=1, x=2} as contributors. Each contributor
        // draws a FRESH degree-2 polynomial (k-1 = 2 higher coefficients) with its share as q(0).
        let k = 3usize;
        let new_xs: [u8; 5] = [1, 2, 3, 4, 5];
        // Distinct fresh coefficient sets per contributor (would be CSPRNG in production).
        let contrib1_c1 = [0x0Fu8, 0x1E, 0x2D, 0x3C, 0x4B, 0x5A, 0x69, 0x78];
        let contrib1_c2 = [0xA1u8, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6, 0x07, 0x18];
        let contrib2_c1 = [0x33u8, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA];
        let contrib2_c2 = [0x1Cu8, 0x2B, 0x3A, 0x49, 0x58, 0x67, 0x76, 0x85];
        let c1_higher: [&[u8]; 2] = [&contrib1_c1, &contrib1_c2];
        let c2_higher: [&[u8]; 2] = [&contrib2_c1, &contrib2_c2];
        assert_eq!(c1_higher.len() + 1, k, "k = (k-1 higher coeffs) + 1");

        // Each new node j gets a sub-share q_i(y_j) from EACH contributor i, tagged with i's OLD x.
        let new_shares: Vec<(u8, Vec<u8>)> = new_xs
            .iter()
            .map(|&y| {
                let sub1 = crate::reshare_eval(&old[0], &c1_higher, y).expect("contrib1 eval"); // x_i = 1
                let sub2 = crate::reshare_eval(&old[1], &c2_higher, y).expect("contrib2 eval"); // x_i = 2
                let share = crate::lagrange_combine_at_zero(&[(1, &sub1), (2, &sub2)])
                    .expect("new node combine")
                    .to_vec();
                (y, share)
            })
            .collect();

        // The new shares differ from the old shares at the same coordinate (a genuinely fresh poly).
        assert_ne!(new_shares[0].1, old[0], "new share at x=1 is on a different polynomial");

        // INVARIANT: ANY THREE of the five new shares reconstruct the EXACT CEK.
        let combine3 = |a: usize, b: usize, c: usize| -> Vec<u8> {
            crate::lagrange_combine_at_zero(&[
                (new_shares[a].0, new_shares[a].1.as_slice()),
                (new_shares[b].0, new_shares[b].1.as_slice()),
                (new_shares[c].0, new_shares[c].1.as_slice()),
            ])
            .expect("combine 3")
            .to_vec()
        };
        assert_eq!(combine3(0, 1, 2), cek, "new shares {{1,2,3}} reconstruct the CEK");
        assert_eq!(combine3(0, 2, 4), cek, "new shares {{1,3,5}} reconstruct the CEK");
        assert_eq!(combine3(2, 3, 4), cek, "new shares {{3,4,5}} reconstruct the CEK");

        // BELOW the NEW quorum: any TWO new shares do NOT reconstruct the CEK (degree-2 needs 3).
        let two = crate::lagrange_combine_at_zero(&[
            (new_shares[0].0, new_shares[0].1.as_slice()),
            (new_shares[1].0, new_shares[1].1.as_slice()),
        ])
        .expect("combine 2")
        .to_vec();
        assert_ne!(two, cek, "two new shares are below the new 3-of-5 quorum");

        // OLD MATERIAL DEAD: mixing OLD shares (on the old degree-1 poly) with the new set yields
        // garbage — a node compromised before the reconfiguration holds nothing useful after.
        let mixed = crate::lagrange_combine_at_zero(&[
            (1, old[0].as_slice()),
            (new_shares[1].0, new_shares[1].1.as_slice()),
            (new_shares[2].0, new_shares[2].1.as_slice()),
        ])
        .expect("mixed combine")
        .to_vec();
        assert_ne!(mixed, cek, "an old share inside a new-set reconstruction is garbage");

        // The NEW node-set identity is distinct (different k AND different membership).
        let vks: Vec<Vec<u8>> = (0..5u8).map(|i| vec![0xC0 ^ i; 40]).collect();
        let vk_refs: Vec<&[u8]> = vks.iter().map(|v| v.as_slice()).collect();
        let new_set_id = crate::threshold_node_set_id_n(3, &vk_refs);
        assert_ne!(
            new_set_id,
            crate::threshold_node_set_id_n(2, &vk_refs[..3]),
            "the reconfigured set has a distinct node-set id (k and m both changed)"
        );

        // The reshare AAD welds (kid, old set, new set, k, m): any field change diverges.
        let kid = [0x5Au8; 16];
        let old_id = crate::threshold_node_set_id_n(2, &vk_refs[..3]);
        let aad = crate::reshare_aad(&kid, &old_id, &new_set_id, 3, 5);
        assert_eq!(aad, crate::reshare_aad(&kid, &old_id, &new_set_id, 3, 5));
        assert_ne!(aad, crate::reshare_aad(&kid, &old_id, &new_set_id, 2, 5), "k is bound");
        assert_ne!(aad, crate::reshare_aad(&kid, &old_id, &new_set_id, 3, 4), "m is bound");
        assert!(aad.starts_with(crate::DKMS_RESHARE_DOMAIN));

        // Fail-closed surface.
        assert!(crate::reshare_eval(&cek, &c1_higher, 0).is_err(), "y=0 is the secret, never a node");
        assert!(crate::reshare_eval(&cek, &[], 1).is_err(), "k=1 (no higher coeffs) is degenerate");
        assert!(crate::lagrange_combine_at_zero(&[]).is_err(), "empty point set is refused");
        assert!(
            crate::lagrange_combine_at_zero(&[(1, &old[0]), (1, &old[1])]).is_err(),
            "duplicate coordinates are not a quorum"
        );
    }

    /// DISTRIBUTED KEY GENERATION (Day 126–130): a 2-of-3 CEK is BORN distributed — three dealers
    /// each draw a fresh degree-1 polynomial with a RANDOM constant term, the CEK is the XOR of the
    /// three contributions, and each member sums the sub-shares routed to it into a final share on
    /// the summed polynomial `F`. Proves: no single dealer's contribution equals the CEK (born
    /// distributed); any TWO final shares reconstruct the EXACT CEK; one share is below quorum; the
    /// CEK binding verifies for the right CEK and rejects a wrong one; the sum + binding fail closed.
    #[test]
    fn dkg_2of3_is_born_distributed_and_any_two_reconstruct() {
        // Three dealers, each a degree-1 polynomial f_i(x) = c_i ⊕ a_i·x. Constant terms c_i are
        // the private contributions; the CEK is their XOR (it is assembled NOWHERE in this flow).
        let contrib: [[u8; 8]; 3] = [
            [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88],
            [0x0F, 0x1E, 0x2D, 0x3C, 0x4B, 0x5A, 0x69, 0x78],
            [0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6, 0x07, 0x18],
        ];
        let higher: [[u8; 8]; 3] = [
            [0x9A, 0xBC, 0xDE, 0xF0, 0x12, 0x34, 0x56, 0x78],
            [0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA],
            [0x1C, 0x2B, 0x3A, 0x49, 0x58, 0x67, 0x76, 0x85],
        ];
        // The CEK = ⊕_i c_i. No party computes this during generation; we compute it here only to
        // assert the reconstruction lands on it.
        let mut cek = [0u8; 8];
        for c in &contrib {
            for (a, &b) in cek.iter_mut().zip(c.iter()) {
                *a ^= b;
            }
        }
        // BORN DISTRIBUTED: no single dealer's contribution is the CEK.
        for c in &contrib {
            assert_ne!(&c[..], &cek[..], "a single dealer contribution must not equal the CEK");
        }

        // Each dealer i evaluates f_i at the three member coordinates x = 1,2,3.
        let xs: [u8; 3] = [1, 2, 3];
        let eval = |i: usize, x: u8| -> Vec<u8> {
            let hi: [&[u8]; 1] = [&higher[i]];
            crate::reshare_eval(&contrib[i], &hi, x).expect("dealer eval")
        };
        // Member j sums the three sub-shares f_i(x_j) into its final share F(x_j).
        let final_shares: Vec<(u8, zeroize::Zeroizing<Vec<u8>>)> = xs
            .iter()
            .map(|&x| {
                let s0 = eval(0, x);
                let s1 = eval(1, x);
                let s2 = eval(2, x);
                let subs: [&[u8]; 3] = [&s0, &s1, &s2];
                (x, crate::dkg_sum_subshares(&subs).expect("dkg sum"))
            })
            .collect();

        // INVARIANT: any TWO final shares reconstruct the EXACT CEK (F is degree 1, t=2).
        let combine = |a: usize, b: usize| -> Vec<u8> {
            crate::lagrange_combine_at_zero(&[
                (final_shares[a].0, final_shares[a].1.as_slice()),
                (final_shares[b].0, final_shares[b].1.as_slice()),
            ])
            .expect("dkg combine")
            .to_vec()
        };
        assert_eq!(combine(0, 1), &cek[..], "members {{1,2}} reconstruct the DKG-born CEK");
        assert_eq!(combine(0, 2), &cek[..], "members {{1,3}} reconstruct the DKG-born CEK");
        assert_eq!(combine(1, 2), &cek[..], "members {{2,3}} reconstruct the DKG-born CEK");

        // Below quorum: one share is just one point of F and reveals nothing of F(0).
        assert_ne!(final_shares[0].1.as_slice(), &cek[..], "a single DKG share is not the CEK");

        // CEK BINDING: a public commitment the producer publishes (it learns the CEK transiently to
        // encrypt content). The boundary re-derives it from the reconstructed CEK; a wrong CEK fails.
        let dkg_id = [0x7Au8; 16];
        let node_set = crate::threshold_node_set_id_n(2, &[&[0xA1u8; 40][..], &[0xB2u8; 40][..], &[0xC3u8; 40][..]]);
        let binding = crate::dkg_cek_binding(&dkg_id, &node_set, &cek);
        assert_eq!(binding, crate::dkg_cek_binding(&dkg_id, &node_set, &combine(0, 1)), "binding verifies for the reconstructed CEK");
        let mut wrong = cek;
        wrong[0] ^= 0x01;
        assert_ne!(binding, crate::dkg_cek_binding(&dkg_id, &node_set, &wrong), "binding rejects a wrong CEK");
        assert_ne!(binding, crate::dkg_cek_binding(&[0x00u8; 16], &node_set, &cek), "binding is ceremony-bound");

        // The DKG AAD welds (kid, dkg_id, node_set, t, m): any field change diverges.
        let kid = [0x5Au8; 16];
        let aad = crate::dkg_aad(&kid, &dkg_id, &node_set, 2, 3);
        assert_eq!(aad, crate::dkg_aad(&kid, &dkg_id, &node_set, 2, 3));
        assert_ne!(aad, crate::dkg_aad(&kid, &dkg_id, &node_set, 3, 3), "t is bound");
        assert_ne!(aad, crate::dkg_aad(&kid, &dkg_id, &node_set, 2, 5), "m is bound");
        assert!(aad.starts_with(crate::DKMS_DKG_DOMAIN));
        // The sub-share AAD welds the dealer→target pair.
        let sub = crate::dkg_subshare_aad(&kid, &dkg_id, &node_set, 1, 2);
        assert_ne!(sub, crate::dkg_subshare_aad(&kid, &dkg_id, &node_set, 2, 2), "dealer coordinate is bound");
        assert_ne!(sub, crate::dkg_subshare_aad(&kid, &dkg_id, &node_set, 1, 3), "target coordinate is bound");

        // Fail-closed surface.
        assert!(crate::dkg_sum_subshares(&[]).is_err(), "empty sub-share set is refused");
        assert!(crate::dkg_sum_subshares(&[&[][..]]).is_err(), "empty sub-shares are refused");
        let short: &[u8] = &[0u8; 4];
        let long: &[u8] = &[0u8; 8];
        assert!(crate::dkg_sum_subshares(&[short, long]).is_err(), "length mismatch is refused");
    }

    /// Day 131–135 — a genuine t-of-n quorum's co-signed release attestations aggregate into a proof
    /// that verifies OFFLINE, names WHICH node-set served the open, and fails closed for every forgery
    /// (under-quorum, wrong-principal, expired, duplicate-padding, forged member, wrong node-set).
    #[test]
    fn quorum_release_proof_verifies_offline_and_fails_closed() {
        // A 2-of-3 node-set: three secret-holders, each with its own master-derived seal keypair.
        let (s0, vk0) = mldsa_seal_keypair([0xA0u8; 32]);
        let (s1, vk1) = mldsa_seal_keypair([0xB1u8; 32]);
        let (_s2, vk2) = mldsa_seal_keypair([0xC2u8; 32]);
        let members: Vec<&[u8]> = vec![&vk0, &vk1, &vk2];
        let t = 2u8;
        let node_set_id = threshold_node_set_id_n(t, &members);

        // The grant + per-open session + expiry every releasing member co-signs.
        let content_id = b"content:matrix-4k".as_slice();
        let principal_id = b"principal:alice".as_slice();
        let right = b"play".as_slice();
        let kid = [0x5Au8; 16];
        let session_pub = [0x77u8; 32]; // a fresh decrypt-session ephemeral pubkey (freshness)
        let expiry = 2_000u64;
        let now = 1_000u64;

        let sign = |signer: &MlDsaSealSigner| {
            sign_release_attestation(signer, content_id, principal_id, right, &node_set_id, &session_pub, &kid, expiry)
        };
        let sig0 = sign(&s0);
        let sig1 = sign(&s1);

        // GENUINE: members {0,1} (a real quorum) → verifies offline, returns the distinct-signer count.
        let proof: Vec<(usize, &[u8])> = vec![(0, &sig0), (1, &sig1)];
        assert_eq!(
            verify_quorum_release_proof(t, &members, &node_set_id, content_id, principal_id, right, &session_pub, &kid, expiry, now, &proof),
            Ok(2),
            "a genuine quorum proof verifies offline"
        );

        // WRONG-PRINCIPAL: same signatures, but a relying party checking for a DIFFERENT principal —
        // the signatures were never over those bytes, so the first counted signer is named as bad.
        assert_eq!(
            verify_quorum_release_proof(t, &members, &node_set_id, content_id, b"principal:mallory", right, &session_pub, &kid, expiry, now, &proof),
            Err(QuorumProofError::BadSignature { member_index: 0 }),
            "a proof bound to alice does not authorize mallory"
        );
        // REPLAYED AGAINST ANOTHER OPEN: a different decrypt session → the freshness binding fails.
        let other_session = [0x88u8; 32];
        assert_eq!(
            verify_quorum_release_proof(t, &members, &node_set_id, content_id, principal_id, right, &other_session, &kid, expiry, now, &proof),
            Err(QuorumProofError::BadSignature { member_index: 0 }),
            "an attestation cannot be replayed against a different open"
        );

        // UNDER-QUORUM: only one valid signature → not a real quorum.
        assert_eq!(
            verify_quorum_release_proof(t, &members, &node_set_id, content_id, principal_id, right, &session_pub, &kid, expiry, now, &[(0, sig0.as_slice())]),
            Err(QuorumProofError::BelowQuorum { have: 1, need: 2 }),
            "one signer is below quorum"
        );
        // DUPLICATE PADDING: one node cannot replay its own signature to fake a quorum.
        assert_eq!(
            verify_quorum_release_proof(t, &members, &node_set_id, content_id, principal_id, right, &session_pub, &kid, expiry, now, &[(0, sig0.as_slice()), (0, sig0.as_slice())]),
            Err(QuorumProofError::DuplicateSigner { member_index: 0 }),
            "a duplicate signer cannot pad the count"
        );
        // EXPIRED: now past the attested expiry.
        assert_eq!(
            verify_quorum_release_proof(t, &members, &node_set_id, content_id, principal_id, right, &session_pub, &kid, expiry, expiry + 1, &proof),
            Err(QuorumProofError::Expired),
            "an aged-out proof is rejected"
        );
        // FORGED MEMBER: an impostor signs but its key is not member[2] — naming member 2 as bad.
        let (imp, _impvk) = mldsa_seal_keypair([0xEEu8; 32]);
        let forged = sign(&imp);
        assert_eq!(
            verify_quorum_release_proof(t, &members, &node_set_id, content_id, principal_id, right, &session_pub, &kid, expiry, now, &[(0, &sig0), (2, &forged)]),
            Err(QuorumProofError::BadSignature { member_index: 2 }),
            "a forged member signature is rejected AND the member is named"
        );
        // WRONG NODE-SET: a proof claiming an id that does not match its (t, members) is rejected.
        let bogus_id = [0x00u8; 32];
        assert_eq!(
            verify_quorum_release_proof(t, &members, &bogus_id, content_id, principal_id, right, &session_pub, &kid, expiry, now, &proof),
            Err(QuorumProofError::NodeSetMismatch),
            "a proof cannot claim a node-set it is not"
        );

        // COMPOSES WITH RECONFIGURATION: the SAME primitives over a reconfigured 3-of-5 set name the
        // CURRENT set (members + t recompute the live id; a 2-of-3 proof would mismatch).
        let (s3, vk3) = mldsa_seal_keypair([0xD3u8; 32]);
        let (s4, vk4) = mldsa_seal_keypair([0xE4u8; 32]);
        let big: Vec<&[u8]> = vec![&vk0, &vk1, &vk2, &vk3, &vk4];
        let big_id = threshold_node_set_id_n(3, &big);
        assert_ne!(big_id, node_set_id, "the reconfigured set has a DISTINCT id");
        let bsign = |signer: &MlDsaSealSigner| {
            sign_release_attestation(signer, content_id, principal_id, right, &big_id, &session_pub, &kid, expiry)
        };
        let b1 = bsign(&s1);
        let b3 = bsign(&s3);
        let b4 = bsign(&s4);
        assert_eq!(
            verify_quorum_release_proof(3, &big, &big_id, content_id, principal_id, right, &session_pub, &kid, expiry, now, &[(1, &b1), (3, &b3), (4, &b4)]),
            Ok(3),
            "a 3-of-5 proof on the reconfigured set verifies and names the current set"
        );

        // EMPTY: no members → nothing to verify against.
        assert_eq!(
            verify_quorum_release_proof(t, &[], &node_set_id, content_id, principal_id, right, &session_pub, &kid, expiry, now, &proof),
            Err(QuorumProofError::EmptyMembers),
        );
        // The preimage is domain-separated and field-bound.
        let msg = release_attestation_message(content_id, principal_id, right, &node_set_id, &session_pub, &kid, expiry);
        assert!(msg.starts_with(DKMS_RELEASE_ATTEST_DOMAIN));
        assert_ne!(
            msg,
            release_attestation_message(content_id, principal_id, b"download", &node_set_id, &session_pub, &kid, expiry),
            "the right is bound"
        );
    }

    /// The n-node node-set id generalization: byte-identical to the 2-node id for n=2 (no
    /// re-pinning on upgrade), and a 3-node set pins ALL THREE members + their order + t.
    #[test]
    fn threshold_node_set_id_n_extends_two_node_id_byte_identically() {
        let vk_a = vec![0xA1u8; 40];
        let vk_b = vec![0xB2u8; 52];
        let vk_c = vec![0xC3u8; 44];
        assert_eq!(
            crate::threshold_node_set_id(2, &vk_a, &vk_b),
            crate::threshold_node_set_id_n(2, &[&vk_a, &vk_b]),
            "the 2-node id must be byte-identical through the n-node derivation"
        );
        let id3 = crate::threshold_node_set_id_n(2, &[&vk_a, &vk_b, &vk_c]);
        assert_ne!(id3, crate::threshold_node_set_id_n(2, &[&vk_a, &vk_b]), "adding a member changes the id");
        assert_ne!(id3, crate::threshold_node_set_id_n(2, &[&vk_a, &vk_c, &vk_b]), "order matters");
        assert_ne!(id3, crate::threshold_node_set_id_n(3, &[&vk_a, &vk_b, &vk_c]), "t is bound");
        let (_s, rogue) = crate::seal::mldsa_seal_keypair([0x99u8; 32]);
        assert_ne!(
            id3,
            crate::threshold_node_set_id_n(2, &[&vk_a, &vk_b, &rogue]),
            "swapping the third member is detectable"
        );
    }

    /// The 2-of-2 node-set identity is deterministic, order-sensitive, and changes if ANY node's
    /// identity changes — so a silently swapped node-set is detectable by comparing the id.
    #[test]
    fn threshold_node_set_id_pins_both_nodes() {
        let vk_a = vec![0xA1u8; 40];
        let vk_b = vec![0xB2u8; 52];
        let id = crate::threshold_node_set_id(2, &vk_a, &vk_b);

        // Deterministic: the same inputs always yield the same id.
        assert_eq!(id, crate::threshold_node_set_id(2, &vk_a, &vk_b));
        // Order matters — (a,b) is a DIFFERENT set-id than (b,a).
        assert_ne!(id, crate::threshold_node_set_id(2, &vk_b, &vk_a));
        // Swapping EITHER node's identity changes the id (a swapped secret-holder is detectable).
        let vk_b2 = vec![0xB3u8; 52];
        assert_ne!(id, crate::threshold_node_set_id(2, &vk_a, &vk_b2));
        let vk_a2 = vec![0xA2u8; 40];
        assert_ne!(id, crate::threshold_node_set_id(2, &vk_a2, &vk_b));
        // The threshold parameter `t` is bound too.
        assert_ne!(id, crate::threshold_node_set_id(3, &vk_a, &vk_b));
        // Length-prefixing prevents a concatenation collision: (vk_a‖x, vk_b) != (vk_a, x‖vk_b).
        let mut a_plus = vk_a.clone();
        a_plus.extend_from_slice(&[0xCCu8; 4]);
        let mut pre_b = vec![0xCCu8; 4];
        pre_b.extend_from_slice(&vk_b);
        assert_ne!(
            crate::threshold_node_set_id(2, &a_plus, &vk_b),
            crate::threshold_node_set_id(2, &vk_a, &pre_b),
            "the length prefix must prevent a boundary-shift collision"
        );
    }

    /// Day 105–108: the encrypted-channel KEY ATTESTATION pins both the handshake challenge AND the
    /// node's channel KEM key under the node identity — the property that defeats a MITM terminating
    /// the TCP connection (it can relay the genuine hello but cannot substitute its own KEM key).
    #[test]
    fn channel_key_attestation_binds_challenge_and_key() {
        let (signer, vk) = mldsa_seal_keypair([0x61u8; 32]);
        let verifier = MlDsa65Verifier::from_encoded(&vk).expect("vk decodes");
        let challenge = [0x10u8; 32];
        let channel_pub = vec![0x42u8; 64];
        let sig = crate::attest_channel_key(&signer, &challenge, &channel_pub);

        // The genuine (challenge, channel_pub) verifies under the pinned identity.
        assert!(crate::verify_channel_key(&verifier, &challenge, &channel_pub, &sig));
        // A SUBSTITUTED channel key (the MITM's own KEM key) fails under the pinned identity.
        let mitm_pub = vec![0x43u8; 64];
        assert!(!crate::verify_channel_key(&verifier, &challenge, &mitm_pub, &sig));
        // A different challenge (replayed attestation) fails.
        assert!(!crate::verify_channel_key(&verifier, &[0x11u8; 32], &channel_pub, &sig));
        // A different node identity fails (the attestation is identity-pinned).
        let (_other, other_vk) = mldsa_seal_keypair([0x62u8; 32]);
        let other_verifier = MlDsa65Verifier::from_encoded(&other_vk).expect("vk decodes");
        assert!(!crate::verify_channel_key(&other_verifier, &challenge, &channel_pub, &sig));
        // Domain separation: a hello attestation over the same challenge is NOT a channel attestation.
        let hello_sig = crate::attest_challenge(&signer, &challenge);
        assert!(!crate::verify_channel_key(&verifier, &challenge, &channel_pub, &hello_sig));
    }

    /// Day 105–108: the per-frame channel AAD separates channel, direction and sequence — a frame
    /// can be neither REFLECTED back at its sender nor REPLAYED once the receiver's counter advances.
    #[test]
    fn channel_frame_aad_separates_channel_direction_and_seq() {
        let id = [0x77u8; 32];
        let aad = crate::channel_frame_aad(&id, 0, 1);
        // Deterministic (node + client must compute byte-identical AADs).
        assert_eq!(aad, crate::channel_frame_aad(&id, 0, 1));
        // Direction-separated: a client→node frame cannot be reflected as a node→client frame.
        assert_ne!(aad, crate::channel_frame_aad(&id, 1, 1));
        // Sequence-separated: a captured frame replayed after the counter advanced fails.
        assert_ne!(aad, crate::channel_frame_aad(&id, 0, 2));
        // Channel-separated: a frame from one handshake cannot cross to another channel.
        assert_ne!(aad, crate::channel_frame_aad(&[0x78u8; 32], 0, 1));
        // Domain-labelled (never collides with another AAD family).
        assert!(aad.starts_with(crate::DKMS_CHANNEL_FRAME_DOMAIN));
    }

    /// Day 109–112: the ROTATION AAD binds the kid, the SOURCE node and the SUCCESSOR node — a
    /// refresh delta sealed for one rotation cannot authorize another (no kid-swap, no source-swap,
    /// no successor-redirect), and the domain never collides with the escrow AAD family.
    #[test]
    fn rotation_aad_binds_kid_source_and_successor() {
        let kid = [0x11u8; 16];
        let source = [0x22u8; 64];
        let successor = [0x33u8; 64];
        let aad = crate::rotation_aad(&kid, &source, &successor);
        // Deterministic (operator + node must compute byte-identical AADs).
        assert_eq!(aad, crate::rotation_aad(&kid, &source, &successor));
        // Kid-separated: a delta minted to rotate one content cannot rotate another.
        assert_ne!(aad, crate::rotation_aad(&[0x12u8; 16], &source, &successor));
        // Source-separated: a delta sealed for node A cannot drive node B's rotation.
        assert_ne!(aad, crate::rotation_aad(&kid, &[0x23u8; 64], &successor));
        // Successor-separated: an attacker cannot REDIRECT the rotated share to its own recipient.
        assert_ne!(aad, crate::rotation_aad(&kid, &source, &[0x34u8; 64]));
        // Domain-labelled, and NEVER the escrow AAD (a rotation delta is not an escrowed share).
        assert!(aad.starts_with(crate::DKMS_ROTATE_DOMAIN));
        assert_ne!(aad, crate::transcript::escrow_aad(SUITE_PQ_HYBRID, &kid, &successor));
    }

    /// Day 109–112: a caller REVOCATION verifies only under the operator identity over the exact
    /// caller key — forged signatures, other callers, other identities, and signatures lifted from
    /// every sibling domain are all refused.
    #[test]
    fn revocation_signature_is_operator_and_caller_bound() {
        let (operator, operator_vk) = mldsa_seal_keypair([0x6Fu8; 32]);
        let verifier = MlDsa65Verifier::from_encoded(&operator_vk).expect("vk decodes");
        let caller_pub = vec![0x55u8; 96];
        let sig = crate::sign_revocation(&operator, &caller_pub);

        // The genuine revocation verifies under the pinned operator identity.
        assert!(crate::verify_revocation(&verifier, &caller_pub, &sig));
        // A DIFFERENT caller is not revoked by this signature.
        assert!(!crate::verify_revocation(&verifier, &[0x56u8; 96], &sig));
        // A non-operator signer cannot revoke (the node pins the operator identity).
        let (impostor, impostor_vk) = mldsa_seal_keypair([0x70u8; 32]);
        let impostor_sig = crate::sign_revocation(&impostor, &caller_pub);
        assert!(!crate::verify_revocation(&verifier, &caller_pub, &impostor_sig));
        let impostor_verifier = MlDsa65Verifier::from_encoded(&impostor_vk).expect("vk decodes");
        assert!(impostor_vk != operator_vk && crate::verify_revocation(&impostor_verifier, &caller_pub, &impostor_sig));
        // Domain separation: a hello attestation / channel attestation over the same bytes is NOT
        // a revocation — no signature from a sibling domain can stand in for one.
        let hello_sig = crate::attest_challenge(&operator, &caller_pub);
        assert!(!crate::verify_revocation(&verifier, &caller_pub, &hello_sig));
        let channel_sig = crate::attest_channel_key(&operator, &caller_pub, &caller_pub);
        assert!(!crate::verify_revocation(&verifier, &caller_pub, &channel_sig));
    }

    /// The escrow AAD both producer and authority bind is deterministic + labelled.
    #[test]
    fn escrow_aad_is_deterministic_and_labelled() {
        let kid = [0x11u8; 16];
        let recip = [0x22u8; 64];
        let a = crate::transcript::escrow_aad(SUITE_PQ_HYBRID, &kid, &recip);
        assert_eq!(a, crate::transcript::escrow_aad(SUITE_PQ_HYBRID, &kid, &recip));
        let label = crate::transcript::ESCROW_AAD_LABEL;
        assert_eq!(&a[..4], &(label.len() as u32).to_be_bytes());
        assert_eq!(&a[4..4 + label.len()], label);
    }

    /// Any escrow field change yields a different AAD — a CEK escrowed for one
    /// {scheme, KID, recipient} cannot be opened as another (re-target / KID-swap defense).
    #[test]
    fn escrow_aad_changes_with_every_field() {
        let kid = [0x11u8; 16];
        let recip = [0x22u8; 64];
        let base = crate::transcript::escrow_aad(SUITE_PQ_HYBRID, &kid, &recip);
        assert_ne!(base, crate::transcript::escrow_aad("other-suite", &kid, &recip));
        let mut kid2 = kid;
        kid2[0] ^= 1;
        assert_ne!(base, crate::transcript::escrow_aad(SUITE_PQ_HYBRID, &kid2, &recip));
        let mut recip2 = recip;
        recip2[0] ^= 1;
        assert_ne!(base, crate::transcript::escrow_aad(SUITE_PQ_HYBRID, &kid, &recip2));
    }

    /// The seal/unwrap path is welded to the shared transcript: a CEK sealed to one
    /// transcript's AAD opens under it and fails closed under any other — proving the
    /// shared encoder is a sufficient contract for the cross-capsule handoff.
    #[test]
    fn transcript_bound_seal_round_trips_and_rejects_mismatch() {
        let (secret, public) = mint_session();
        let (signer, vk) = mldsa_seal_keypair(SEED);
        let verifier = MlDsa65Verifier::from_encoded(&vk).expect("verifier");
        let aad = sample_transcript().to_aad();

        let env = seal_bound(&public, &cek(), &aad, &signer);
        let opened = hybrid_unwrap_bound(&secret, &env, &aad, &verifier).expect("matching opens");
        assert_eq!(opened.as_slice(), &cek());

        // A different transcript changes the signed payload (`payload ‖ aad`), so the
        // signature check fails closed first — before any KEM/AEAD work.
        let mut other = sample_transcript();
        other.principal_id = "did:elastos:mallory";
        assert_eq!(
            hybrid_unwrap_bound(&secret, &env, &other.to_aad(), &verifier),
            Err(PqEnvelopeError::BadSignature),
            "a different transcript must fail closed"
        );
    }

    fn sample_receipt_hash() -> [u8; 32] {
        crate::transcript::release_receipt_hash(
            "elastos.release.receipt/v1",
            "key-release:1",
            "bafyobject",
            "did:elastos:alice",
            "sess-1",
            "decrypt",
            "key-provider",
            "released",
            1_800_000_000,
            1_900_000_000,
        )
    }

    /// The receipt hash both rail sides bind is deterministic from equal fields.
    #[test]
    fn receipt_hash_is_deterministic() {
        assert_eq!(sample_receipt_hash(), sample_receipt_hash());
    }

    /// Any receipt field change yields a different hash (so a swapped receipt can't be
    /// slid under an existing transcript binding).
    #[test]
    fn receipt_hash_changes_with_every_field() {
        let base = sample_receipt_hash();
        let changed = crate::transcript::release_receipt_hash(
            "elastos.release.receipt/v1",
            "key-release:1",
            "bafyobject",
            "did:elastos:alice",
            "sess-1",
            "decrypt",
            "key-provider",
            "denied", // status flipped
            1_800_000_000,
            1_900_000_000,
        );
        assert_ne!(base, changed, "a flipped status must change the receipt hash");
        let later_expiry = crate::transcript::release_receipt_hash(
            "elastos.release.receipt/v1",
            "key-release:1",
            "bafyobject",
            "did:elastos:alice",
            "sess-1",
            "decrypt",
            "key-provider",
            "released",
            1_800_000_000,
            1_900_000_001, // expiry +1
        );
        assert_ne!(base, later_expiry, "a changed expiry must change the receipt hash");
    }
}
