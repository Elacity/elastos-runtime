#!/usr/bin/env python3
"""Cross-language golden weld: assert the Python canonical construction matches the Rust one.

These EXACT values are also asserted in `capsules/ddrm-envelope/src/av.rs`
(`canonical_golden_vectors`). If either side changes the construction, BOTH test suites fail —
that is the anti-drift guarantee for the serve-time selector (Rust) ↔ forensic extractor (Python).

Pure stdlib — runnable in CI without numpy/ffmpeg:  python3 tools/av-forensics/test_canonical.py
"""
import sys
from canonical import (
    asset_bias_vector,
    bias_commitment_hex,
    buyer_codeword,
    grant_watermark_digest16,
    interleave_map,
    tardos_score,
)

# GOLDEN (baked from the verified ddrm-envelope run; identical on both sides):
GOLDEN_BIAS = [48909, 54388, 58492, 37588, 5543, 43201, 3392, 1311, 1311, 54695,
               40770, 34251, 10820, 26744, 64224, 50072]
GOLDEN_CODE = [1, 0, 1, 1, 1, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 0]
GOLDEN_INTERLEAVE = [2, 5, 1, 11, 15, 13, 3, 0, 8, 14, 10, 12, 6, 9, 7, 4]
GOLDEN_BIAS_COMMIT = "7099bd8a809e791263ee2b01fa6e04910cfec7cbd1a5e1d0de4b3c9e9e93acfc"
GOLDEN_GRANT = bytes([11, 236, 138, 200, 38, 183, 221, 175, 22, 38, 113, 248, 66, 90, 154, 54])


def main() -> int:
    bias = asset_bias_vector(b"av-golden-asset", 16)
    grant = grant_watermark_digest16("0xgolden")
    code = buyer_codeword(grant, bias)
    inter = interleave_map(b"av-golden-asset", 16)

    checks = [
        ("grant_watermark_digest16", grant, GOLDEN_GRANT),
        ("asset_bias_vector", bias, GOLDEN_BIAS),
        ("buyer_codeword", code, GOLDEN_CODE),
        ("interleave_map", inter, GOLDEN_INTERLEAVE),
        ("bias_commitment_hex", bias_commitment_hex(bias), GOLDEN_BIAS_COMMIT),
    ]
    ok = True
    for name, got, want in checks:
        if got == want:
            print(f"  ok    {name}")
        else:
            ok = False
            print(f"  DRIFT {name}\n        got  {got}\n        want {want}")

    # Sanity: distinct grants -> distinct codewords, and the leaker scores highest.
    c_other = buyer_codeword(grant_watermark_digest16("0xfeedface"), bias)
    if c_other == code:
        ok = False
        print("  DRIFT distinct grants produced identical codewords")
    else:
        print("  ok    distinct grants -> distinct codewords")

    keep = [True] * 16
    rows = [buyer_codeword(grant_watermark_digest16(f"0xbuyer{j}"), bias) for j in range(6)]
    leaker = 4
    scores = [tardos_score(rows[leaker], rows[j], bias, keep) for j in range(6)]
    if max(range(6), key=lambda j: scores[j]) == leaker:
        print("  ok    tardos_score names the (noiseless) leaker")
    else:
        ok = False
        print(f"  DRIFT tardos_score mis-ranked: {scores}")

    print("PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
