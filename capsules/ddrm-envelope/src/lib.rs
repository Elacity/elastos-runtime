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
            v
        }
    }
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

/// GF(2^8) multiply (AES polynomial 0x11B). Constant pattern, no tables.
fn gf256_mul(mut a: u8, mut b: u8) -> u8 {
    let mut acc = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 {
            acc ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1B;
        }
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
pub const DKMS_RECOVER_DOMAIN: &[u8] = b"elastos.dkms.authority/recover-proof/v1";

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
/// AND a per-recover FRESHNESS counter (`recover_seq`, Day 95–96). Binding the recover identity means
/// the proof authorizes recovering THIS content for THIS session; binding a strictly-increasing
/// `recover_seq` means a captured recover frame replayed verbatim carries a STALE counter the node
/// has already consumed, so it is refused (anti-replay). The runtime-core analogue of PC2's
/// per-delegation revocable `nonce` (`secureViewSession.ts:108`–`:112`). Defined ONCE here so the
/// node + client cannot drift.
pub fn recover_proof_message(
    challenge: &[u8],
    content_id: &[u8],
    kid_hex: &[u8],
    decrypt_session_pub: &[u8],
    recover_seq: u64,
) -> Vec<u8> {
    lp_concat(
        DKMS_RECOVER_DOMAIN,
        &[challenge, content_id, kid_hex, decrypt_session_pub, &recover_seq.to_le_bytes()],
    )
}

/// The CLIENT side: prove possession of the token-bound ephemeral private key by signing the recover
/// binding + this recover's freshness counter. The node verifies this against the pubkey the session
/// token committed to AND that `recover_seq` strictly advances (a replayed frame is refused).
pub fn sign_recover_proof(
    signer: &impl seal::CekSealSigner,
    challenge: &[u8],
    content_id: &[u8],
    kid_hex: &[u8],
    decrypt_session_pub: &[u8],
    recover_seq: u64,
) -> Vec<u8> {
    signer.sign(&recover_proof_message(challenge, content_id, kid_hex, decrypt_session_pub, recover_seq))
}

/// The NODE side: verify the caller's possession proof against the token-bound pubkey. `true` only
/// when `sig` is valid under `verifier` (built from the token's `caller_pub`) over the SAME binding
/// (including `recover_seq`) — a missing/forged proof, a proof from a different key, a tampered
/// binding, or a swapped freshness counter all return `false`. Freshness (the strictly-increasing
/// check) is enforced by the node against its per-session counter, not here.
pub fn verify_recover_proof(
    verifier: &impl CekSealVerifier,
    challenge: &[u8],
    content_id: &[u8],
    kid_hex: &[u8],
    decrypt_session_pub: &[u8],
    recover_seq: u64,
    sig: &[u8],
) -> bool {
    verifier.verify(
        &recover_proof_message(challenge, content_id, kid_hex, decrypt_session_pub, recover_seq),
        sig,
    )
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
        let proof = crate::sign_recover_proof(&caller, &challenge, content, kid, sess_pub, seq);
        assert!(crate::verify_recover_proof(&caller_verifier, &challenge, content, kid, sess_pub, seq, &proof));

        // A proof from a DIFFERENT key (a captured-token replayer without the private key) fails.
        let (other, _ovk) = crate::seal::mldsa_seal_keypair([0x62u8; 32]);
        let wrong = crate::sign_recover_proof(&other, &challenge, content, kid, sess_pub, seq);
        assert!(!crate::verify_recover_proof(&caller_verifier, &challenge, content, kid, sess_pub, seq, &wrong));

        // A tampered binding (different content / kid / session pub / challenge / freshness seq) fails.
        assert!(!crate::verify_recover_proof(&caller_verifier, &challenge, b"bafOTHER", kid, sess_pub, seq, &proof));
        assert!(!crate::verify_recover_proof(&caller_verifier, &challenge, content, b"ffff", sess_pub, seq, &proof));
        assert!(!crate::verify_recover_proof(&caller_verifier, &challenge, content, kid, b"otherpub", seq, &proof));
        assert!(!crate::verify_recover_proof(&caller_verifier, b"otherchal", content, kid, sess_pub, seq, &proof));
        // A SWAPPED freshness counter invalidates the proof (the seq is authenticated, not free to alter).
        assert!(!crate::verify_recover_proof(&caller_verifier, &challenge, content, kid, sess_pub, seq + 1, &proof));

        // The possession proof is domain-separated from the session token (different domain prefix).
        assert!(!crate::verify_session_token(&caller_verifier, &challenge, content, 0, &proof));

        // A malformed signature fails closed.
        assert!(!crate::verify_recover_proof(&caller_verifier, &challenge, content, kid, sess_pub, seq, b"nope"));
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
