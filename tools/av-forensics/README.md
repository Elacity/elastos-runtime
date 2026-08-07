# AV forensic extractor (offline) — AV Phase 5, chunk 6

The **offline forensic tool** that names the buyer behind a leaked A/V copy: it recovers the
per-segment variant pattern out of a leaked file and decodes it back to the access-grant identity
that released the key. This is the proven Phase-0/5 research reference, ported in-tree.

It runs **offline, by an operator, on an already-leaked public file** — it touches no key material,
is not in the serve hot path, and is not in the decrypt boundary. That is why it is Python (the
proven reference) rather than a blind Rust port; the Rust `--extract-av-fingerprint` CLI is deferred
until the codeword scheme is frozen/certified (see `../../docs/AV_WATERMARKING.md`).

## The cross-language weld (this is the load-bearing part)

`canonical.py` is the **byte-for-byte mirror** of the Rust serve-time selector
`capsules/ddrm-envelope/src/av.rs`. The serve side (Rust) chooses each buyer's variant sequence;
this extractor (Python) decodes it. If they disagree, attribution is meaningless — so both assert
the **same golden vectors**:

```bash
python3 tools/av-forensics/test_canonical.py     # pure stdlib, no numpy/ffmpeg — CI-able
```

`test_canonical.py` ↔ `ddrm-envelope::av::tests::canonical_golden_vectors`. Change the construction
on either side and **both** fail.

## Files

| File | Role | Deps |
|---|---|---|
| `canonical.py` | Canonical codeword: bias vector, buyer row, interleave, Tardos score, **analytic threshold** (mirror of `av.rs`) | stdlib |
| `test_canonical.py` | Golden weld vs the Rust side + threshold sanity (the smoke) | stdlib |
| `montecarlo.py` | **FP/FN certification sweep** of the analytic threshold across six collusion strategies | numpy |
| `fm_register.py` | Deterministic Fourier-Mellin registration (informed; valid-region resolution) | numpy |
| `dsp.py` | Spread-spectrum embed + differential detect + ffmpeg encode/decode/leak | numpy + ffmpeg |
| `extractor.py` | End-to-end: grant → codeword → embed → leak → register → recover → accuse | numpy + ffmpeg |

## Running the full extractor

```bash
python3 -m venv .venv && . .venv/bin/activate
pip install -r tools/av-forensics/requirements.txt    # numpy; also needs `ffmpeg` on PATH
python3 tools/av-forensics/extractor.py                # self-test (synthesizes a master)
python3 tools/av-forensics/extractor.py master.gray    # real: pass the published raw-gray master
```

For real forensics: supply the asset's published master frames and replace the demo candidate list
with the asset's audit-log grant digests (`grant_watermark_digest16`), and use the asset's stored
bias vector. The extractor demonstrates *with* vs *without* registration to show registration is
what makes attribution survive a geometric leak.

## Honest envelope (do not overclaim — mirrors the doc)

- **Crop / zoom / bounded shift / re-frame: recovered** (informed FM, valid-region resolution).
- **Rotation: OUT of envelope** — needs a proper rotation estimator (the rarest leak).
- **Audio + the VMAF figure: synthetic-only so far** — real-content + real-screen-record
  re-validation (incl. PEAQ/ODG, human A/B/X, time-stretch/pitch) is mandatory before any claim.
- **Accusation is now FP-controlled (code level).** `extractor.py` accuses only above the
  **analytic** Tardos threshold `canonical.tardos_threshold` (`Z = √m·Φ⁻¹(1−ε/N)`), not an
  empirical gap. `montecarlo.py` (2000 trials, `m=2332 N=500 c=3 ε=1e-3 BER=0.13`) confirms
  **FP ≤ ε with 100% detection across six collusion strategies**; the old `mean+3.5σ` ran 2–10×
  over ε. This certifies the **accusation statistics only** — *not* media survival.
- **Media survival is still uncertified:** real-content + real-screen-record re-validation
  (audio incl. PEAQ/ODG + human A/B/X, real CMAF segment lengths) is mandatory before any
  end-to-end claim, and short clips remain too thin for a certified `c`.

```bash
python3 tools/av-forensics/montecarlo.py            # default sweep (needs numpy)
python3 tools/av-forensics/montecarlo.py --c 4 --trials 4000   # stress a larger collusion
```
