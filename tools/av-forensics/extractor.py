#!/usr/bin/env python3
"""End-to-end offline forensic extractor (AV Phase 5, chunk 6) — the proven reference.

Chain: grant_digest -> canonical Tardos codeword -> embed per-segment variant marks -> encode
-> LEAK (crop + re-encode) -> FM-register to master -> differential per-segment detect
-> recover codeword (low-confidence segments = erasures) -> Tardos accuse -> NAME the buyer.

The codeword/score math is the CANONICAL construction (`canonical.py`), byte-identical to the Rust
serve selector (`ddrm-envelope::av`) — so a copy leaked by the runtime decodes back here. Run as a
self-test it synthesizes a master; for REAL forensics pass the master raw-gray file as argv[1] and
adapt the candidate grant list to the asset's audit-log grant digests.

Requires numpy + a system `ffmpeg`. Honest envelope (see ../../docs/AV_WATERMARKING.md):
  * crop/zoom/bounded-shift: recovered (informed FM, valid-region resolution);
  * rotation: OUT of envelope (needs a proper estimator);
  * the single-leaker gap test below is a DEMO, not a certified accusation — production collusion
    needs the analytic Tardos threshold + a Monte-Carlo FP/FN sweep, and short clips are too thin.
"""
import os
import sys

import numpy as np

from canonical import (
    asset_bias_vector,
    buyer_codeword,
    grant_watermark_digest16,
    tardos_score,
)
from dsp import N, SEG, H, W, decode, detect, embed, encode, ff_transform, load_gray
from fm_register import register

N_BUYERS = 8
ERASURE_TAU = 2.0                       # per-segment |z| below this -> erasure (untrusted symbol)
ASSET_SECRET = b"av-forensics-demo-asset"


def codeword_to_frame_bits(codeword):
    """Tardos row (0/1 per segment) -> per-frame antipodal sign (+1 for 1, -1 for 0)."""
    sign = np.array(codeword, dtype=float) * 2 - 1
    return np.repeat(sign, SEG)[:N]


def recover(frames, master, est, nseg):
    """Per-segment symbol + confidence from a (registered) leaked clip."""
    z = detect(frames, master, est=est)
    bits, conf = [], []
    for s in range(nseg):
        segz = z[s * SEG:(s + 1) * SEG].sum() / np.sqrt(SEG)
        bits.append(1 if segz > 0 else 0)
        conf.append(abs(float(segz)))
    return bits, conf


def accuse(y, conf, rows, bias_q):
    """Tardos-score every candidate over non-erased positions; return (scores, keep_mask)."""
    keep = [c >= ERASURE_TAU for c in conf]
    scores = [tardos_score(y, rows[j], bias_q, keep) for j in range(len(rows))]
    return scores, keep


def _synth_master():
    """A synthetic 'published' clip when no real master is supplied (self-test mode)."""
    rng = np.random.default_rng(20240601)
    base = rng.integers(0, 256, size=(H, W)).astype(np.float64)
    return np.stack([np.clip(base + rng.normal(0, 6, (H, W)), 0, 255) for _ in range(N)])


def main():
    if len(sys.argv) > 1 and os.path.exists(sys.argv[1]):
        master = load_gray(sys.argv[1])
        print(f"master: {sys.argv[1]}")
    else:
        master = _synth_master()
        print("master: synthetic (self-test mode; pass a raw-gray master as argv[1] for real use)")

    nseg = N // SEG
    bias_q = asset_bias_vector(ASSET_SECRET, nseg)
    grants = [grant_watermark_digest16(f"0xbuyer{j}") for j in range(N_BUYERS)]
    rows = [buyer_codeword(g, bias_q) for g in grants]

    leaker = 3
    marked = embed(master, codeword_to_frame_bits(rows[leaker]))
    encode(marked, "leakpub.mp4", crf=23)
    leaked = decode(ff_transform("leakpub.mp4", "crop10"))   # leak: crop+zoom then re-encode

    print(f"true leaker = buyer {leaker};  candidates = {N_BUYERS};  segments(=m) = {nseg}\n")
    for label, est in [
        ("NO registration", None),
        ("FM registration", register(leaked[N // 2], master[N // 2])[1]),
    ]:
        y, conf = recover(leaked, master, est, nseg)
        scores, keep = accuse(y, conf, rows, bias_q)
        acc = int(np.argmax(scores))
        second = sorted(scores)[-2]
        named = acc if scores[acc] > 3.0 * max(second, 0.5) else None
        bit_err = sum(1 for i in range(nseg) if keep[i] and y[i] != rows[leaker][i])
        verdict = f"NAMED buyer {named}" if named == leaker else f"mis/none: {named}"
        print(f"{label:<16}: erasures={sum(1 for k in keep if not k)}/{nseg}  "
              f"bitERR(vs leaker)={bit_err}  top=buyer {acc} "
              f"(score {scores[acc]:.1f}, next {second:.1f})  -> {verdict}")


if __name__ == "__main__":
    main()
