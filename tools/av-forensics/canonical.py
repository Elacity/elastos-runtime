#!/usr/bin/env python3
"""Canonical AV forensic-variant codeword construction — the cross-language weld.

This is the byte-for-byte mirror of `capsules/ddrm-envelope/src/av.rs` (the Rust serve-time
selector). The serve side (Rust) and this offline extractor (Python) MUST derive identical
per-buyer codewords from a grant digest, or attribution is meaningless. So every derivation is a
domain-separated SHA-256 stream over integers — no RNG, no float in the bit decision. The only
float (`sin`) is used once to build the per-asset bias vector, whose *quantized* result is shared.

`test_canonical.py` asserts the same golden vectors as the Rust `canonical_golden_vectors` test;
if either side drifts, both fail. Pure stdlib (hashlib + math) — no numpy/ffmpeg needed.
"""
import hashlib
import math

AV_VARIANTS_SCHEMA = "elastos.ddrm.av-variants/v1"

BIAS_QUANT = 65_535
BIAS_CLAMP_LO = 0.02
BIAS_CLAMP_HI = 0.98

_DOMAIN_BIAS = b"elastos.av.tardos.bias/v1"
_DOMAIN_ROW = b"elastos.av.tardos.row/v1"
_DOMAIN_INTERLEAVE = b"elastos.av.interleave/v1"


def grant_watermark_digest16(delegation_sig_hex: str) -> bytes:
    """16-byte SHA-256 prefix over the normalised (trim + lowercase) delegation-signature hex.
    Mirror of `ddrm_envelope::grant_watermark_digest16`."""
    normalized = delegation_sig_hex.strip().lower()
    return hashlib.sha256(normalized.encode()).digest()[:16]


def _prf_u32(domain: bytes, key: bytes, index: int) -> int:
    """u32::from_be_bytes(SHA-256(domain ‖ key ‖ index_be)[..4]) — the canonical per-index word."""
    d = hashlib.sha256(domain + key + index.to_bytes(4, "big")).digest()
    return int.from_bytes(d[0:4], "big")


def asset_bias_vector(asset_secret: bytes, m: int) -> list[int]:
    """Per-asset quantized Tardos bias vector p_q[i] (u16). Computed once; this vector IS the
    per-asset secret. `round-half-away-from-zero` via floor(x+0.5) to match Rust's f64::round."""
    out = []
    for i in range(m):
        u = _prf_u32(_DOMAIN_BIAS, asset_secret, i) / (2 ** 32)
        r = u * (math.pi / 2.0)
        p = min(max(math.sin(r) ** 2, BIAS_CLAMP_LO), BIAS_CLAMP_HI)
        out.append(int(math.floor(p * BIAS_QUANT + 0.5)))
    return out


def bias_commitment_hex(bias_q: list[int]) -> str:
    """hex(SHA-256(bias_q as little-endian u16 bytes)) — the commitment stored in the manifest."""
    h = hashlib.sha256()
    for v in bias_q:
        h.update(int(v).to_bytes(2, "little"))
    return h.hexdigest()


def buyer_codeword(grant_digest: bytes, bias_q: list[int]) -> list[int]:
    """Deterministic buyer row: bit[i] = 1 iff a per-(grant,i) 16-bit PRF value < p_q[i]."""
    return [
        1 if (_prf_u32(_DOMAIN_ROW, grant_digest, i) & 0xFFFF) < pq else 0
        for i, pq in enumerate(bias_q)
    ]


def interleave_map(asset_secret: bytes, m: int) -> list[int]:
    """Permutation of 0..m: codeword position k -> marked-segment slot. Sort by (prf, index)
    (stable) so consecutive code bits land on non-adjacent segments."""
    return sorted(range(m), key=lambda i: (_prf_u32(_DOMAIN_INTERLEAVE, asset_secret, i), i))


def tardos_score(y: list[int], x: list[int], bias_q: list[int], keep: list[bool]) -> float:
    """Symmetric Tardos score of candidate row x against recovered word y over kept positions.
    Use with `tardos_threshold` for an FP-controlled accusation — never `argmax` alone."""
    n = min(len(y), len(x), len(bias_q), len(keep))
    s = 0.0
    for i in range(n):
        if not keep[i]:
            continue
        p = bias_q[i] / BIAS_QUANT
        a = math.sqrt((1.0 - p) / p)
        b = math.sqrt(p / (1.0 - p))
        if y[i] == 1:
            s += a if x[i] == 1 else -b
        else:
            s += -a if x[i] == 1 else b
    return s


def _inv_norm_cdf(p: float) -> float:
    """Inverse standard-normal CDF (Acklam's rational approximation, |abs err| < 1.15e-9).
    Pure stdlib — `math` has no erfinv."""
    if not 0.0 < p < 1.0:
        raise ValueError("p must be in (0,1)")
    a = [-3.969683028665376e+01, 2.209460984245205e+02, -2.759285104469687e+02,
         1.383577518672690e+02, -3.066479806614716e+01, 2.506628277459239e+00]
    b = [-5.447609879822406e+01, 1.615858368580409e+02, -1.556989798598866e+02,
         6.680131188771972e+01, -1.328068155288572e+01]
    c = [-7.784894002430293e-03, -3.223964580411365e-01, -2.400758277161838e+00,
         -2.549732539343734e+00, 4.374664141464968e+00, 2.938163982698783e+00]
    d = [7.784695709041462e-03, 3.224671290700398e-01, 2.445134137142996e+00,
         3.754408661907416e+00]
    plow, phigh = 0.02425, 1 - 0.02425
    if p < plow:
        q = math.sqrt(-2 * math.log(p))
        return ((((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5])
                / ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1))
    if p <= phigh:
        q = p - 0.5
        r = q * q
        return ((((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) * q
                / (((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1))
    q = math.sqrt(-2 * math.log(1 - p))
    return -((((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5])
             / ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1))


def tardos_threshold(m: int, n_users: int, eps: float) -> float:
    """Analytic FP-controlled accusation threshold for the symmetric Tardos score.

    Under innocence the per-position score contribution has mean 0 and variance EXACTLY 1
    (a²p + b²(1-p) = (1-p) + p = 1, independent of the bias p), so a kept-length-`m` innocent
    score is ~ N(0, m) by the CLT. Bonferroni over `n_users` candidates at a TOTAL false-accusation
    budget `eps`:  Z = sqrt(m) · Φ⁻¹(1 − eps/n_users).

    This replaces the Phase-0 empirical `mean + kσ` threshold (which Monte-Carlo showed was ~1.25%
    FP — not certifiable). `m` is the number of KEPT (non-erased) positions actually scored.
    """
    if m <= 0:
        raise ValueError("m (kept positions) must be > 0")
    alpha = eps / n_users
    return math.sqrt(m) * _inv_norm_cdf(1.0 - alpha)
