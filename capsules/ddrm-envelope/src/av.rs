//! AV forensic-variant layer — variant manifest schema (chunk 1) + the canonical, RNG-free
//! codeword derivation (chunk 2).
//!
//! Boundary discipline (see `docs/AV_WATERMARKING.md`): variant selection is server-side, keyed by
//! the *signed grant* (`grant_watermark_digest16`); the CEK never leaves the decrypt VM; one
//! canonical serve path (absence of variants ⇒ `fingerprinted:false`, never a hidden unmarked
//! fast-path).
//!
//! Why "canonical, RNG-free": the serve-time selector (Rust) and the offline forensic extractor
//! (the proven Python reference under `tools/av-forensics/`) must derive **the same** per-buyer
//! codeword from a grant digest, or attribution is meaningless. So every derivation here is a
//! domain-separated SHA-256 stream over integers — no language's RNG, no float in the bit decision.
//! The only float (`sin`) is used **once at mint** to build the per-asset bias vector, whose
//! *quantized* result is stored and shared; the serve/extract paths only ever read those integers.
//! `tools/av-forensics/canonical.py` mirrors this byte-for-byte and the golden vectors below are
//! asserted identically on both sides (the `grant_watermark_digest16` anti-drift pattern).

use sha2::{Digest, Sha256};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Schema id for the variant manifest. Bumped on any wire-format change.
pub const AV_VARIANTS_SCHEMA: &str = "elastos.ddrm.av-variants/v1";

// ----------------------------------------------------------------------------------------------
// chunk 1 — variant manifest schema (`elastos.ddrm.av-variants/v1`)
// ----------------------------------------------------------------------------------------------

/// The per-asset variant manifest, published in the DASH directory alongside the (now per-variant)
/// CENC segments. Describes which segments carry variants, the variant set per marked segment, the
/// per-variant embedded symbol, and the codeword scheme (length / interleave / erasure policy).
/// `fingerprinted == false` is the explicit, fail-closed honest state: the asset has a single
/// encode and carries no per-buyer mark (never claim a mark that isn't there).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct VariantManifestV1 {
    /// Must equal [`AV_VARIANTS_SCHEMA`].
    pub schema: String,
    /// `false` ⇒ single encode, no variants, no attribution (honest).
    pub fingerprinted: bool,
    /// Variant arity: 2 = A/B, 4 = q-ary A/B/C/D (the density lever).
    pub arity: u8,
    /// The anti-collusion codeword scheme bound to this asset.
    pub codeword: CodewordScheme,
    /// The marked subset, in timeline order. `len() == codeword.length` when fingerprinted.
    pub marked_segments: Vec<MarkedSegment>,
}

/// The codeword scheme: length, the timeline interleave permutation, the erasure threshold, and a
/// commitment to the (server-secret) per-asset bias vector — so the manifest binds the exact biases
/// the extractor must use, without publishing the secret itself.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct CodewordScheme {
    /// `m` — number of marked segments = codeword length.
    pub length: u32,
    /// Permutation of `0..length`: codeword position `k` → marked-segment slot (timeline
    /// interleaving so a bursty whole-segment loss becomes scattered erasures, not a lost run).
    pub interleave: Vec<u32>,
    /// Per-segment recovered `|z|` below which the symbol is an **erasure** (not a forced bit).
    pub erasure_tau: f64,
    /// `hex(SHA-256(bias_q little-endian u16 bytes))` — commits to the per-asset bias vector
    /// (the secret stays server-side; this lets the extractor prove it used the right one).
    pub bias_commitment_hex: String,
}

/// One marked segment: its timeline index and the per-symbol variant references.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct MarkedSegment {
    /// Segment index in the DASH timeline.
    pub index: u32,
    /// One entry per arity symbol (`symbol` ∈ `0..arity`).
    pub variants: Vec<VariantRef>,
}

/// A single variant of a marked segment: which symbol it encodes, its CENC segment URI, and the
/// segment digest welded into the transcript AAD (chunk 4 — the full-variant-set weld).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct VariantRef {
    /// The embedded symbol this variant carries (`0 = A`, `1 = B`, …).
    pub symbol: u8,
    /// The CENC-encrypted segment file for this variant (relative to the DASH dir).
    pub uri: String,
    /// `hex` of the segment digest bound into the AAD (substituting an out-of-set segment fails
    /// the CEK unwrap closed).
    pub digest_hex: String,
}

impl VariantManifestV1 {
    /// An honest single-encode (unfingerprinted) manifest — the fail-closed default when an asset
    /// has no variants.
    pub fn single_encode() -> Self {
        VariantManifestV1 {
            schema: AV_VARIANTS_SCHEMA.to_string(),
            fingerprinted: false,
            arity: 1,
            codeword: CodewordScheme {
                length: 0,
                interleave: Vec::new(),
                erasure_tau: 0.0,
                bias_commitment_hex: String::new(),
            },
            marked_segments: Vec::new(),
        }
    }

    /// Structural validation — fail closed on any internal inconsistency so a malformed manifest
    /// can never be served as if it were a valid mark.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema != AV_VARIANTS_SCHEMA {
            return Err("unknown variant manifest schema");
        }
        if !self.fingerprinted {
            // Unfingerprinted ⇒ no marks at all.
            if !self.marked_segments.is_empty() || self.codeword.length != 0 {
                return Err("unfingerprinted manifest must carry no marked segments");
            }
            return Ok(());
        }
        if self.arity < 2 {
            return Err("a fingerprinted manifest needs arity >= 2");
        }
        if self.marked_segments.len() != self.codeword.length as usize {
            return Err("codeword length must equal the marked-segment count");
        }
        if self.codeword.interleave.len() != self.codeword.length as usize {
            return Err("interleave length must equal the codeword length");
        }
        if !is_permutation(&self.codeword.interleave) {
            return Err("interleave must be a permutation of 0..length");
        }
        for seg in &self.marked_segments {
            if seg.variants.len() != self.arity as usize {
                return Err("each marked segment must carry exactly `arity` variants");
            }
            for (want, v) in seg.variants.iter().enumerate() {
                if v.symbol as usize != want {
                    return Err("variant symbols must be 0..arity in order");
                }
            }
        }
        Ok(())
    }
}

fn is_permutation(xs: &[u32]) -> bool {
    let mut seen = vec![false; xs.len()];
    for &x in xs {
        match seen.get_mut(x as usize) {
            Some(slot) if !*slot => *slot = true,
            _ => return false,
        }
    }
    seen.into_iter().all(|s| s)
}

// ----------------------------------------------------------------------------------------------
// chunk 2 — canonical codeword derivation (RNG-free; mirrored in tools/av-forensics/canonical.py)
// ----------------------------------------------------------------------------------------------

/// Quantization denominator: a Tardos bias `p ∈ [0,1]` is stored as `round(p * BIAS_QUANT)` (u16).
pub const BIAS_QUANT: u32 = 65_535;
/// Arcsine-law bias clamp (Tardos `t`-clipping), matching the reference harness.
pub const BIAS_CLAMP_LO: f64 = 0.02;
/// Arcsine-law bias clamp (Tardos `t`-clipping), matching the reference harness.
pub const BIAS_CLAMP_HI: f64 = 0.98;

const DOMAIN_BIAS: &[u8] = b"elastos.av.tardos.bias/v1";
const DOMAIN_ROW: &[u8] = b"elastos.av.tardos.row/v1";
const DOMAIN_INTERLEAVE: &[u8] = b"elastos.av.interleave/v1";

/// `u32::from_be_bytes(SHA-256(domain ‖ key ‖ index_be)[..4])` — the canonical per-index PRF word.
fn prf_u32(domain: &[u8], key: &[u8], index: u32) -> u32 {
    let mut h = Sha256::new();
    h.update(domain);
    h.update(key);
    h.update(index.to_be_bytes());
    let d = h.finalize();
    u32::from_be_bytes([d[0], d[1], d[2], d[3]])
}

/// Per-asset **quantized** Tardos bias vector `p_q[i]`, computed ONCE at mint from the per-asset
/// secret. This vector *is* the per-asset secret (kept server-side); the serve selector and the
/// extractor read the stored integers and never re-derive the float. `sin` is used only here.
pub fn asset_bias_vector(asset_secret: &[u8], m: u32) -> Vec<u16> {
    (0..m)
        .map(|i| {
            // u ∈ [0,1): divide by 2^32 so the full PRF word maps uniformly.
            let u = prf_u32(DOMAIN_BIAS, asset_secret, i) as f64 / (u32::MAX as f64 + 1.0);
            let r = u * std::f64::consts::FRAC_PI_2;
            let sin_r = r.sin();
            let p = (sin_r * sin_r).clamp(BIAS_CLAMP_LO, BIAS_CLAMP_HI);
            (p * BIAS_QUANT as f64).round() as u16
        })
        .collect()
}

/// `hex(SHA-256(bias_q as little-endian u16 bytes))` — the commitment stored in the manifest.
pub fn bias_commitment_hex(bias_q: &[u16]) -> String {
    let mut h = Sha256::new();
    for &v in bias_q {
        h.update(v.to_le_bytes());
    }
    let d = h.finalize();
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Deterministic buyer codeword row from a grant digest + the per-asset bias vector:
/// `bit[i] = 1` iff a per-`(grant,i)` PRF value (16-bit) `< p_q[i]`. Pure integer ⇒ the Rust serve
/// selector and the Python extractor derive identical rows. No per-buyer storage — recomputed from
/// the `grant_watermark_digest16` already in the custody log.
pub fn buyer_codeword(grant_digest: &[u8], bias_q: &[u16]) -> Vec<u8> {
    bias_q
        .iter()
        .enumerate()
        .map(|(i, &pq)| {
            let v = prf_u32(DOMAIN_ROW, grant_digest, i as u32) & 0xFFFF;
            u8::from(v < pq as u32)
        })
        .collect()
}

/// Timeline interleave permutation of `0..m`: codeword position `k` → marked-segment slot. Positions
/// are ordered by `(prf_key, index)` so consecutive codeword bits land on non-adjacent segments —
/// a bursty whole-segment loss becomes scattered erasures, not a contiguous lost run. Deterministic
/// (stable tie-break on the index), so Rust and Python agree.
pub fn interleave_map(asset_secret: &[u8], m: u32) -> Vec<u32> {
    let mut order: Vec<u32> = (0..m).collect();
    order.sort_by_key(|&i| (prf_u32(DOMAIN_INTERLEAVE, asset_secret, i), i));
    order
}

/// Symmetric Tardos score of candidate row `x` against the recovered word `y`, summed over the kept
/// (non-erased) positions, using the dequantized bias. Higher ⇒ more likely the leaker.
///
/// NOTE: this is the *score* only. A certified accusation needs the **analytic Tardos threshold +
/// a Monte-Carlo FP/FN sweep** (still open — see `docs/AV_WATERMARKING.md`); callers must not treat
/// `argmax` alone as proof.
pub fn tardos_score(y: &[u8], x: &[u8], bias_q: &[u16], keep: &[bool]) -> f64 {
    let n = y.len().min(x.len()).min(bias_q.len()).min(keep.len());
    let mut s = 0.0_f64;
    for i in 0..n {
        if !keep[i] {
            continue;
        }
        let p = bias_q[i] as f64 / BIAS_QUANT as f64;
        let a = ((1.0 - p) / p).sqrt();
        let b = (p / (1.0 - p)).sqrt();
        s += match (y[i], x[i]) {
            (1, 1) => a,
            (1, _) => -b,
            (0, 1) => -a,
            _ => b,
        };
    }
    s
}

// ----------------------------------------------------------------------------------------------
// chunk 3 — serve-time variant selection · chunk 4 — full-variant-set AAD weld input
// ----------------------------------------------------------------------------------------------

const DOMAIN_VARIANT_SET: &[u8] = b"elastos.av.variant-set/v1";

/// Serve-time variant selection (chunk 3). Given the per-asset bias vector (the server-side secret,
/// whose commitment the manifest pins) and a buyer's grant digest, return the symbol to serve at
/// each marked segment, in the manifest's timeline order — symbol `s` selects
/// `marked_segments[i].variants[s]`. **Fail-closed:** a malformed manifest, a bias vector that does
/// not match the manifest's `bias_commitment_hex` (wrong per-asset secret), or an unsupported arity
/// all return `Err`, so a copy can never be served *as a mark* unless the exact committed scheme is
/// used. An unfingerprinted (single-encode) manifest returns an empty selection (`fingerprinted:false`).
///
/// Mapping is direct (codeword position `i` ↔ marked segment `i`), matching the proven extractor
/// (`tools/av-forensics`); the manifest's `interleave` is carried + committed for timeline-robustness,
/// but applying it across embed/select/recover is a single tracked follow-up that must land on BOTH
/// the Rust selector and the Python extractor together (see `docs/AV_WATERMARKING.md`). Arity-2 (A/B)
/// only for now — for arity 2 the codeword bit IS the symbol, the quantity already welded by the
/// `canonical_golden_vectors` codeword golden.
pub fn select_symbols(
    manifest: &VariantManifestV1,
    bias_q: &[u16],
    grant_digest: &[u8],
) -> Result<Vec<u8>, &'static str> {
    manifest.validate()?;
    if !manifest.fingerprinted {
        return Ok(Vec::new());
    }
    if manifest.arity != 2 {
        return Err("serve selector supports arity 2 (A/B) only for now");
    }
    if bias_q.len() != manifest.codeword.length as usize {
        return Err("bias vector length does not match the codeword length");
    }
    // Bind the server secret to the published manifest: the provided bias must be the one committed.
    if bias_commitment_hex(bias_q) != manifest.codeword.bias_commitment_hex {
        return Err("bias commitment mismatch — wrong per-asset secret for this manifest");
    }
    Ok(buyer_codeword(grant_digest, bias_q))
}

/// Commitment to the FULL published variant set (chunk 4 weld input): SHA-256 over a domain-separated,
/// length-prefixed encoding of the arity and every marked segment's `(index, per-symbol digest_hex)`
/// in manifest order. Bound into the decrypt transcript AAD so the decrypt boundary confirms it is
/// operating on the exact published set the serve side selected from — substituting/forging a variant
/// outside the set, or swapping the manifest, changes this commitment and fails the CEK unwrap closed.
/// Deterministic + portable (no float, length-prefixed) so an auditor can recompute it from the
/// published manifest.
pub fn variant_set_commitment(manifest: &VariantManifestV1) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(DOMAIN_VARIANT_SET);
    h.update([manifest.arity]);
    h.update((manifest.marked_segments.len() as u32).to_be_bytes());
    for seg in &manifest.marked_segments {
        h.update(seg.index.to_be_bytes());
        h.update((seg.variants.len() as u32).to_be_bytes());
        for v in &seg.variants {
            h.update([v.symbol]);
            let d = v.digest_hex.as_bytes();
            h.update((d.len() as u32).to_be_bytes());
            h.update(d);
        }
    }
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_manifest() -> VariantManifestV1 {
        let interleave = interleave_map(b"asset-x", 3);
        VariantManifestV1 {
            schema: AV_VARIANTS_SCHEMA.to_string(),
            fingerprinted: true,
            arity: 2,
            codeword: CodewordScheme {
                length: 3,
                interleave,
                erasure_tau: 2.0,
                bias_commitment_hex: bias_commitment_hex(&asset_bias_vector(b"asset-x", 3)),
            },
            marked_segments: (0..3)
                .map(|seg| MarkedSegment {
                    index: seg,
                    variants: vec![
                        VariantRef {
                            symbol: 0,
                            uri: format!("seg{seg}.A.m4s"),
                            digest_hex: format!("a{seg}"),
                        },
                        VariantRef {
                            symbol: 1,
                            uri: format!("seg{seg}.B.m4s"),
                            digest_hex: format!("b{seg}"),
                        },
                    ],
                })
                .collect(),
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn manifest_round_trips_serde_and_validates() {
        let m = fixture_manifest();
        m.validate().expect("fixture is valid");
        let json = serde_json::to_string(&m).unwrap();
        let back: VariantManifestV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back, "manifest must round-trip through serde unchanged");
        back.validate().expect("round-tripped manifest is valid");
    }

    #[test]
    fn single_encode_manifest_is_valid_and_honest() {
        let m = VariantManifestV1::single_encode();
        assert!(!m.fingerprinted);
        m.validate().expect("single-encode manifest is valid");
    }

    #[test]
    fn validate_rejects_bad_arity_and_non_permutation() {
        let mut m = fixture_manifest();
        m.codeword.interleave = vec![0, 0, 1]; // not a permutation
        assert!(m.validate().is_err());
        let mut m2 = fixture_manifest();
        m2.arity = 2;
        m2.marked_segments[0].variants.pop(); // arity mismatch
        assert!(m2.validate().is_err());
    }

    #[test]
    fn distinct_grants_yield_distinct_codewords() {
        let bias = asset_bias_vector(b"asset-x", 256);
        let g1 = crate::grant_watermark_digest16("0xdeadbeef");
        let g2 = crate::grant_watermark_digest16("0xfeedface");
        let c1 = buyer_codeword(&g1, &bias);
        let c2 = buyer_codeword(&g2, &bias);
        assert_ne!(c1, c2, "different grants must produce different codewords");
        // deterministic: same grant ⇒ same codeword.
        assert_eq!(c1, buyer_codeword(&g1, &bias));
    }

    #[test]
    fn selector_picks_the_codeword_symbol_per_marked_segment() {
        let bias = asset_bias_vector(b"asset-x", 3);
        let grant = crate::grant_watermark_digest16("0xdeadbeef");
        let manifest = fixture_manifest();
        let symbols = select_symbols(&manifest, &bias, &grant).expect("valid selection");
        assert_eq!(symbols.len(), manifest.marked_segments.len());
        // arity-2: the selection is exactly the welded codeword.
        assert_eq!(symbols, buyer_codeword(&grant, &bias));
        for s in &symbols {
            assert!((*s as usize) < manifest.arity as usize, "symbol in range");
        }
        // deterministic: same grant ⇒ same selection.
        assert_eq!(symbols, select_symbols(&manifest, &bias, &grant).unwrap());
    }

    #[test]
    fn selector_fails_closed_on_wrong_secret_and_unsupported_arity() {
        let manifest = fixture_manifest();
        let grant = crate::grant_watermark_digest16("0xdeadbeef");
        // Wrong per-asset secret ⇒ bias commitment mismatch ⇒ refuse to serve as a mark.
        let wrong_bias = asset_bias_vector(b"a-different-asset", 3);
        assert!(select_symbols(&manifest, &wrong_bias, &grant).is_err());
        // Right secret, wrong length ⇒ refuse.
        let short_bias = asset_bias_vector(b"asset-x", 2);
        assert!(select_symbols(&manifest, &short_bias, &grant).is_err());
        // Single-encode ⇒ empty selection, never an error.
        let honest = VariantManifestV1::single_encode();
        let empty_bias: Vec<u16> = Vec::new();
        assert!(select_symbols(&honest, &empty_bias, &grant)
            .unwrap()
            .is_empty());
        // Unsupported arity (q-ary) ⇒ refuse rather than mis-select.
        let mut qary = fixture_manifest();
        qary.arity = 4;
        assert!(select_symbols(&qary, &asset_bias_vector(b"asset-x", 3), &grant).is_err());
    }

    #[test]
    fn variant_set_commitment_is_stable_and_change_sensitive() {
        let manifest = fixture_manifest();
        let c0 = variant_set_commitment(&manifest);
        assert_eq!(c0, variant_set_commitment(&manifest), "deterministic");
        // Flipping any published variant digest changes the commitment (out-of-set ⇒ AAD fails closed).
        let mut tampered = manifest.clone();
        tampered.marked_segments[1].variants[0].digest_hex.push('f');
        assert_ne!(
            c0,
            variant_set_commitment(&tampered),
            "set commitment must bind every digest"
        );
        // A single-encode manifest commits to an empty set distinct from a fingerprinted one.
        assert_ne!(
            c0,
            variant_set_commitment(&VariantManifestV1::single_encode())
        );
    }

    #[test]
    fn interleave_is_a_permutation() {
        let map = interleave_map(b"asset-x", 64);
        assert_eq!(map.len(), 64);
        assert!(is_permutation(&map));
    }

    #[test]
    fn tardos_score_names_the_leaker() {
        // Small end-to-end on the Rust construction (mirrors tools/av-forensics/extractor):
        // build rows for N buyers, leak buyer L's row through a noisy channel, score everyone.
        let m = 96u32;
        let bias = asset_bias_vector(b"asset-leak", m);
        let n_buyers = 8usize;
        let rows: Vec<Vec<u8>> = (0..n_buyers)
            .map(|j| buyer_codeword(&crate::grant_watermark_digest16(&format!("0xbuyer{j}")), &bias))
            .collect();
        let leaker = 3usize;
        // channel: flip ~12% of the leaker's bits (the spike's worst plausible per-mark BER).
        let mut y = rows[leaker].clone();
        for (i, b) in y.iter_mut().enumerate() {
            if prf_u32(b"test.channel.flip", b"asset-leak", i as u32) % 100 < 12 {
                *b ^= 1;
            }
        }
        let keep = vec![true; m as usize];
        let scores: Vec<f64> = (0..n_buyers)
            .map(|j| tardos_score(&y, &rows[j], &bias, &keep))
            .collect();
        let top = (0..n_buyers).max_by(|&a, &b| scores[a].total_cmp(&scores[b])).unwrap();
        assert_eq!(top, leaker, "the leaker must score highest: {scores:?}");
    }

    /// Cross-language golden weld: these exact bytes are asserted identically in
    /// `tools/av-forensics/test_canonical.py`. If either side changes the construction, both fail.
    #[test]
    fn canonical_golden_vectors() {
        let bias = asset_bias_vector(b"av-golden-asset", 16);
        let grant = crate::grant_watermark_digest16("0xgolden");
        let code = buyer_codeword(&grant, &bias);
        let inter = interleave_map(b"av-golden-asset", 16);

        // GOLDEN (baked from a verified run; mirrored in Python):
        assert_eq!(
            bias,
            GOLDEN_BIAS,
            "bias vector golden drift"
        );
        assert_eq!(code, GOLDEN_CODE, "codeword golden drift");
        assert_eq!(inter, GOLDEN_INTERLEAVE, "interleave golden drift");
        assert_eq!(bias_commitment_hex(&bias), GOLDEN_BIAS_COMMIT, "bias commitment drift");
    }

    // --- golden constants (baked from a verified run; mirrored in tools/av-forensics) ---
    const GOLDEN_BIAS: [u16; 16] = [
        48909, 54388, 58492, 37588, 5543, 43201, 3392, 1311, 1311, 54695, 40770, 34251, 10820,
        26744, 64224, 50072,
    ];
    const GOLDEN_CODE: [u8; 16] = [1, 0, 1, 1, 1, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 0];
    const GOLDEN_INTERLEAVE: [u32; 16] = [2, 5, 1, 11, 15, 13, 3, 0, 8, 14, 10, 12, 6, 9, 7, 4];
    const GOLDEN_BIAS_COMMIT: &str =
        "7099bd8a809e791263ee2b01fa6e04910cfec7cbd1a5e1d0de4b3c9e9e93acfc";

    /// Prints the golden values so they can be baked above (and into the Python test). Run with:
    /// `cargo test -p ddrm-envelope --features av-variants print_goldens -- --nocapture --ignored`
    #[test]
    #[ignore]
    fn print_goldens() {
        let bias = asset_bias_vector(b"av-golden-asset", 16);
        let grant = crate::grant_watermark_digest16("0xgolden");
        let code = buyer_codeword(&grant, &bias);
        let inter = interleave_map(b"av-golden-asset", 16);
        eprintln!("BIAS = {bias:?}");
        eprintln!("CODE = {code:?}");
        eprintln!("INTERLEAVE = {inter:?}");
        eprintln!("BIAS_COMMIT = {}", bias_commitment_hex(&bias));
        eprintln!("GRANT(0xgolden) = {grant:?}");
    }
}
