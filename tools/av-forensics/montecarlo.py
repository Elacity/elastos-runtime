#!/usr/bin/env python3
"""Monte-Carlo FP/FN validation of the analytic Tardos threshold (AV Phase 5 certification gate).

Closes the open item the Phase-0 harness exposed: its empirical `mean + 3.5σ` threshold was ~1.25%
false-accusation (not certifiable). This sweeps the ANALYTIC threshold
`Z = sqrt(m)·Φ⁻¹(1−eps/N)` (canonical.py) across MULTIPLE collusion strategies and reports, for
each, the expected innocents falsely accused vs detection — at the analytic Z AND at the old
empirical Z, so the fix is visible side by side.

Scope (honest): this validates the CODE-LEVEL accusation statistics only. Media survival (real
content, real screen-record, real CMAF segment lengths) is a SEPARATE certification that needs real
media — see docs/AV_WATERMARKING.md §7. Statistical model: a buyer row is Bernoulli(p_i), which the
canonical PRF construction realizes. Requires numpy.

  python3 tools/av-forensics/montecarlo.py [--m 2332 --n 500 --c 3 --eps 1e-3 --ber 0.13 --trials 400]
"""
import argparse

import numpy as np

from canonical import BIAS_QUANT, asset_bias_vector, tardos_threshold

STRATEGIES = ["random", "majority", "minority", "all_ones", "all_zeros", "interleave"]


def collude(colluder_rows, strategy, rng):
    """Form the pirate word from the colluders under the marking assumption (agree → that bit;
    differ → the strategy decides). Tests beyond the Phase-0 'random-on-differing' only."""
    R = colluder_rows
    c, _ = R.shape
    same = (R == R[0]).all(0)
    y = R[0].copy()
    diff = ~same
    idx = np.where(diff)[0]
    if strategy == "random":
        y[idx] = (rng.random(idx.size) < 0.5).astype(np.int8)
    elif strategy == "majority":
        y[idx] = (R[:, idx].mean(0) >= 0.5).astype(np.int8)
    elif strategy == "minority":
        y[idx] = (R[:, idx].mean(0) < 0.5).astype(np.int8)
    elif strategy == "all_ones":
        y[idx] = 1
    elif strategy == "all_zeros":
        y[idx] = 0
    elif strategy == "interleave":
        y[idx] = R[idx % c, idx]            # position-dependent colluder
    return y.astype(np.int8)


def score_all(rows, y, p):
    """Vectorized symmetric Tardos score of every candidate row (n,m) against y (m,)."""
    a = np.sqrt((1 - p) / p)
    b = np.sqrt(p / (1 - p))
    contrib = np.where(y[None, :] == 1, np.where(rows == 1, a, -b), np.where(rows == 1, -a, b))
    return contrib.sum(1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--m", type=int, default=2332)     # tight Tardos for c=3, N=500, eps=1e-3
    ap.add_argument("--n", type=int, default=500)
    ap.add_argument("--c", type=int, default=3)
    ap.add_argument("--eps", type=float, default=1e-3)
    ap.add_argument("--ber", type=float, default=0.13)
    ap.add_argument("--erasure", type=float, default=0.0)
    ap.add_argument("--trials", type=int, default=400)
    ap.add_argument("--seed", type=int, default=12345)
    args = ap.parse_args()

    p = np.asarray(asset_bias_vector(b"mc-asset", args.m), dtype=float) / BIAS_QUANT
    rng = np.random.default_rng(args.seed)

    print(f"m={args.m} N={args.n} c={args.c} eps={args.eps} BER={args.ber} "
          f"erasure={args.erasure} trials={args.trials}")
    print(f"Z_analytic = sqrt(m)·Φ⁻¹(1−eps/N) = {tardos_threshold(args.m, args.n, args.eps):.2f}\n")
    print(f"{'strategy':<11} | {'analytic Z':^24} | {'empirical mean+3.5σ':^24}")
    print(f"{'':<11} | {'innocents/trial':>15} {'detect':>7} | {'innocents/trial':>15} {'detect':>7}")
    print("-" * 74)

    for strat in STRATEGIES:
        inno_a = det_a = inno_e = det_e = 0
        for _ in range(args.trials):
            rows = (rng.random((args.n, args.m)) < p).astype(np.int8)
            y = collude(rows[:args.c], strat, rng)
            y = np.where(rng.random(args.m) < args.ber, 1 - y, y).astype(np.int8)
            keep = rng.random(args.m) >= args.erasure
            mkeep = int(keep.sum())
            scores = score_all(rows[:, keep], y[keep], p[keep])

            z_a = tardos_threshold(mkeep, args.n, args.eps)
            z_e = scores.mean() + 3.5 * scores.std()      # the Phase-0 empirical threshold
            inno = scores[args.c:]
            coll = scores[:args.c]
            inno_a += int((inno > z_a).sum())
            inno_e += int((inno > z_e).sum())
            det_a += int((coll > z_a).any())
            det_e += int((coll > z_e).any())

        t = args.trials
        print(f"{strat:<11} | {inno_a / t:>15.5f} {det_a / t:>7.3f} | "
              f"{inno_e / t:>15.5f} {det_e / t:>7.3f}")

    print("-" * 74)
    print(f"FP target: innocents/trial should be ≤ eps = {args.eps} at the analytic Z "
          "(Bonferroni: E[innocents accused] = N·(eps/N) = eps).")
    print("note: CODE-LEVEL statistics only — media survival is a separate certification "
          "(real content/screen-record).")


if __name__ == "__main__":
    main()
