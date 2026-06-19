# Audio & Video Forensic Watermarking — Design & Roadmap

> Direction + design for the AV branch of forensic watermarking. This is a
> **roadmap item, not shipped behaviour.** For what is shipped today (rasterizable
> types) see [PROTECTED_CONTENT.md](PROTECTED_CONTENT.md); for the tier model see
> [ASSET_TIERS.md](ASSET_TIERS.md). For *how* we land non-trivial work see
> [../CLAUDE.md](../CLAUDE.md); for *what is correct* see [../PRINCIPLES.md](../PRINCIPLES.md).

---

## 1. The honest status quo

Today audio/video are the **stream-decrypt** tier, not pixel-lock:

```
encrypted-at-rest  →  CEK released only via 2-of-3 quorum (key-provider)
                   →  decrypted segment-by-segment INSIDE the decrypt VM
                   →  raw decrypted segment handed to the browser MSE SourceBuffer
```

- The CEK/IV **never leave the boundary**; the ordered segment set is welded into
  the transcript AAD (`to_aad_with_segments`), so a substituted/reordered fragment
  fails the CEK unwrap closed before any byte is decrypted (`viewer_open::open_quorum_media`
  → `ddrm-media-authority --quorum --dash-dir`).
- But the **decrypted media bytes do leave** — they must, for the browser to play
  them — and they carry **no per-buyer frame/sample mark**. A screen-record or an
  MSE-buffer scrape yields a clean, unattributable copy.

This is exactly PC2's model and the realistic **browser ceiling without EME/Widevine**.
Calling AV "watermarked" today would be overclaiming. It is *key-protected*, not
*traceable*.

### Why the image path doesn't just transfer

The rasterizable tier marks **inside the boundary at egress** (`watermark::finalize`)
because it controls the final pixels and emits a single flattened JPEG per page. AV
can't reuse that:

- A real frame/sample mark means **decode → mark → re-encode per segment** in the
  hot streaming path — CPU-heavy, and it **breaks CENC byte-alignment** (the AAD weld
  + per-segment auth all assume the published ciphertext). You'd be re-encrypting on
  the fly and rebuilding the transcript per buyer per play. Fragile and slow.
- Doing it at **mint** instead (encode once, mark once) gives every buyer the *same*
  mark — useless for attribution.

So AV needs a fundamentally different shape: **variant selection**, decided at mint,
chosen per buyer at serve, with the heavy work paid once.

---

## 2. Threat model (what we are actually defending)

| Adversary | Capability | In scope? |
|-----------|-----------|-----------|
| Casual re-sharer | screen-records or scrapes the MSE buffer, re-uploads | **Yes — primary** |
| Colluding buyers | a few buyers diff their copies to strip the mark | Yes — variant scheme must resist small collusion |
| Re-encoder | transcodes/crops/re-compresses before leaking | Yes — mark must survive lossy re-encode |
| Boundary breaker | extracts the CEK from the decrypt VM | **No** — that's the dKMS/quorum threat model, not watermarking |

Goal: a leaked copy decodes back to **the buyer whose access grant released the key**
(the same identity the visible+invisible image marks already carry: `wallet · content · time`).

---

## 3. Chosen approach

### 3.1 Video → A/B forensic variant watermarking (industry standard)

The technique premium VOD (studios/Netflix-class) actually uses:

1. **At mint / transcode**, produce **two encodes of each segment** — `A` and `B` —
   carrying different embedded watermark bits (a robust, imperceptible spatial/temporal
   mark per variant). Both are CENC-encrypted and published in the DASH directory. The
   CEK/transcript AAD binds the **complete published variant set** (extending
   `to_aad_with_segments`), so no variant can be substituted or injected; per-buyer
   selection is **post-unwrap routing** over already-authenticated ciphertext.
2. **At serve**, each buyer is handed a **unique A/B sequence** across segments
   (e.g. segment 0→A, 1→B, 2→B, 3→A, …). The sequence is a codeword derived
   deterministically from the buyer's access-grant identity.
3. **On a leak**, decode the variant pattern out of the recovered file → the codeword
   → the buyer. Robust, streaming-compatible, survives re-encode and screen-record,
   and resists limited collusion when the codeword uses an **anti-collusion code**
   (Tardos / ECC fingerprinting) rather than a raw identity hash.

Why it fits *this* runtime:

- The expensive variant production is paid **once at mint** (a transcode-pipeline
  feature), not per play.
- Serve-time is just **variant routing** — the quorum-media path already serves
  segments one-by-one (`ddrm-media-authority` over the descriptor/`segment` protocol);
  it gains a per-segment **variant selector** keyed by the grant. The CEK boundary is
  untouched.
- It degrades honestly: if variants aren't published for an asset, the path serves the
  single encode and **reports "not fingerprinted"** rather than pretending.

### 3.2 Audio → spread-spectrum / echo-hiding mark

Two families apply:

- **In-boundary, image-style** (decode→mark→re-encode per segment): possible, but it
  **breaks CENC alignment** and adds per-segment DSP in the hot path — the same trap
  as §1. Only viable for short/non-streamed audio, if at all.
- **A/B variant at mint** (mirror §3.1): produce two psychoacoustically-masked encodes
  per segment (spread-spectrum or echo-hiding bits), serve a per-buyer variant sequence.
  **This is the recommended path** — it reuses the §3.1 machinery end-to-end and keeps
  CENC/AAD intact.

**Honest call:** scope audio variant marking *alongside* video (§3.1) — same pipeline,
same serve-time selector, different per-variant DSP.

### 3.3 Cheap interim deterrent (near-free, weak)

A **visible per-session identity overlay** rendered by the player (the buyer's
`wallet · time`, faint, like the image mark). Defeated by cropping and does nothing
against an MSE scrape — but it's a visible "you are identified" signal and costs almost
nothing. Ship it as a deterrent **only**, clearly labelled non-forensic, never as the
real control.

### 3.4 Channel coding — required, not optional

The leak channel is **bursty**: re-encode and segment/packet loss take out *whole
segments* (contiguous runs of marks), not i.i.d. bits. So the codeword **must**:

- **interleave across the timeline** — spread each buyer's codeword bits over
  non-adjacent segments so a lost run becomes scattered erasures rather than a lost
  block; and
- use an **erasure-aware code** — mark dropped/low-confidence segments as *erasures*,
  not forced bit decisions, layered under the anti-collusion code.

This is a hard requirement on the codeword design (§5 chunk 2) and the robustness harness
(§5 chunk 6), not a tuning afterthought. The Phase-0 harness modelled only i.i.d.
bit-flips and so understates burst damage.

---

## 4. Architecture (where each piece lives)

```
MINT (one-time, off the hot path)
  producer media ──► transcode pipeline ──► per-segment {A,B} variants
                                            each CENC-encrypted, published in DASH dir
                                            + variant manifest (which segments have variants)

SERVE (per open, hot path — CEK boundary UNCHANGED)
  access grant (buyer identity) ──► anti-collusion codeword
                                    └─► per-segment variant selector
  ddrm-media-authority --quorum --dash-dir
     ├─ recover CEK 2-of-3 (unchanged; threshold-with-grace)
     ├─ for segment i: pick variant[i] = codeword[i]   ← NEW
     └─ CENC-decrypt the SELECTED variant → MSE         (CEK stays in-VM)

FORENSICS (offline)
  leaked file ──► variant-pattern extractor ──► codeword ──► buyer
                  (mirrors `decrypt-provider --extract-watermark` for images)
```

Boundary discipline (must hold — see §7):

- The **variant selector is a provider/runtime concern**, keyed by the *signed access
  grant*, not by anything the browser sends. The capsule/player never chooses its own
  variant (that would be ambient authority + a strip vector).
- The **CEK never leaves the decrypt VM**; variant selection happens *before* decrypt,
  on already-published ciphertext. No on-the-fly re-encryption. The transcript AAD binds
  the **full published variant set**, so per-buyer selection is *routing*, not a new
  authority — a substituted or injected variant fails the unwrap closed.
- **One canonical path**: the quorum-media serve path gains a selector; there is no
  second "unmarked fast path." If variants are absent, the path serves the single
  encode and the descriptor says `fingerprinted: false` — explicit, not silent.

---

## 5. Phasing (each chunk has a one-sentence pass/fail check)

> Per [CLAUDE.md](../CLAUDE.md): smallest independently-verifiable steps. These are
> roadmap chunks; each becomes its own approved plan + tests before code.

### Phase 0 — feasibility study (DONE, off-tree throwaway harness)

> A research-grade, off-tree Python harness (never part of the repo) answered the
> go/no-go before any pipeline work. **Verdict: GO**, with two scoped engineering items
> (geometric registration; code/redundancy sizing). The results below are on **synthetic
> content at sub-segment lengths** and are a feasibility signal, **not** a production
> validation — every number must be re-earned on real media at real CMAF segment lengths
> (see chunk 6 and §7).

**Video** (one bit per 8-frame segment; real CMAF segments are 8–22× longer → coherent
gain ≈ √(len) is stronger — *but only for an attenuated-but-present mark*; a mark
quantized away by heavy re-encode is not recovered by length alone):

| Leak path | seg BER | verdict |
|---|---|---|
| publish encode | 0.000 | GREEN |
| HiDPI screen-record (2× resample) | 0.067 | GREEN |
| transcode → VP9 | 0.000 | GREEN |
| re-encode @ ~½ bitrate | 0.133 | AMBER (needs ECC budget to certify GREEN) |
| re-encode @ ~¼ bitrate | 0.600 | RED (mark quantized away — redundancy *not proven* to recover) |
| crop 10% (geometric) | 0.333 | RED (registration — gating Phase-5 item) |

Imperceptibility **VMAF 96.7** — measured on **480×270 synthetic content with a
1080p-tuned model**, so treat it as a soft *relative* signal, **not the gate**.

**Audio** (0.25 s segments, through AAC 96k / MP3 128k / 44.1k resample / gain): uniform
**seg BER 0.083 (GREEN)**, codec-robust. **Not yet validated:** the test content is
**pink noise** (stationary, broadband — the *easiest* case for both masking and codec
survival), and imperceptibility is a **nominal −42 dB proxy, not a psychoacoustic
measurement**. Real music/speech/silence and time-stretch/pitch are untested (chunk 6).

**Registration (the one genuine research item):** bounded **translation is solved** (a
16 px shift recovers GREEN via windowed FFT cross-correlation). **Scale/zoom is not**:
both a coarse raw-max search and a fine, PSR-selected coherent-sum search (~17 scales)
made detection *worse* — every extra hypothesis is another chance for a spurious peak.
Blind geometric sync needs a **deterministic estimator** (embedded template/pilot, or a
periodic-carrier **log-polar / Fourier–Mellin** method), not a search loop (§5 chunk 5).

**Collusion (Tardos, anchored to `grant_digest`):** the full chain works — `grant_digest`
→ deterministic Tardos row → variant selection → leak at the spike's BER → accusation
naming colluders, with **no per-buyer codeword storage** (the row is recomputed from the
audit-log `grant_digest`; the server stores only a per-asset bias vector). Tight (Škorić
symmetric) length `m = 2π²·c²·ln(N/ε) ≈ 2332` for c=3, N=500, ε=1e-3.
**Correction (audit) — now RESOLVED at code level:** the original harness used an
*empirical* threshold (`mean+3.5σ`); a Monte-Carlo sweep showed it gives **~1.25%
false-accusation — not ε=1e-3**. The fix is landed: `canonical.tardos_threshold` is the
**analytic** `Z = √m·Φ⁻¹(1−ε/N)`, and `tools/av-forensics/montecarlo.py` (2000 trials)
now confirms **FP ≤ ε with 100% detection across all six collusion strategies**
(random / majority / minority / all-ones / all-zeros / interleave) — the empirical
threshold reproduces the failure at **2–10× over ε** (majority worst, ≈1.05%). What
remains is **not** code-level: re-validating the per-asset duration bound on **real
media** at the FP-controlled threshold (real content / screen-record / CMAF lengths).

**Duration bound (provisional, pre-FP-recompute):** long-form (≥~20 min @ 1 mark/s, or
~10 min with q-ary A/B/C/D) supports c=3; short clips (2–4 min ≈ 120–240 marks) support
single-leaker tracing or c=2 only — and the FP-controlled threshold pushes these minimums
**up**, not down.

### Phase 5 — landed so far (chunks 1, 2, 6)

> Canonical chunk numbering follows [`PHASE5_BUILD_SPEC`](AV_WATERMARKING.md) (1 schema · 2 codeword
> · 3 mint · 4 AAD-weld · 5 serve · 6 extractor · 7 overlay). The numbered list below predates that
> spec; the ✅ markers map the two. **Landed = chunks 1, 2, 6** (the tractable, pipeline-free pieces
> built on the proven research). **Mine, deferred to the live CENC/DASH/quorum pipeline = chunks 3
> (mint transcode DSP), 4 (full-variant-set AAD weld), 5 (serve-time selector).**

- **Chunk 1 — variant manifest schema.** `elastos.ddrm.av-variants/v1` lives in
  `capsules/ddrm-envelope/src/av.rs` (`VariantManifestV1`, behind the `av-variants` feature):
  marked-subset, per-symbol variant refs (+ segment digest for the chunk-4 weld), arity (A/B or
  q-ary), and the codeword scheme (length, interleave map, erasure τ, bias commitment). Round-trips
  serde; `validate()` fails closed on any inconsistency; `single_encode()` is the honest
  `fingerprinted:false` default.
- **Chunk 2 — canonical, RNG-free codeword.** Same module: `asset_bias_vector` (quantized arcsine
  Tardos biases — the per-asset secret), `buyer_codeword` (deterministic row from
  `grant_watermark_digest16`, no per-buyer storage), `interleave_map` (timeline interleaving), and
  `tardos_score`. **Decision baked in:** the construction is a domain-separated SHA-256 stream over
  integers — **not** any language's RNG — so the serve selector (Rust) and the extractor (Python)
  derive *identical* codewords. Cross-language **golden vectors** are asserted on both sides
  (`av::tests::canonical_golden_vectors` ↔ `tools/av-forensics/test_canonical.py`) — the
  `grant_watermark_digest16` anti-drift pattern.
- **Accusation is FP-controlled (code-level — landed).** `canonical.tardos_threshold` is the
  **analytic** threshold `Z = √m · Φ⁻¹(1 − ε/N)` — the innocent symmetric-Tardos score is exactly
  mean-0, variance-1 per kept position ⇒ `N(0,m)` — replacing the Phase-0 empirical `mean+kσ`. A
  Monte-Carlo sweep (`tools/av-forensics/montecarlo.py`, **2000 trials**, `m=2332 N=500 c=3 ε=1e-3
  BER=0.13`) confirms **FP ≤ ε with 100% detection across six collusion strategies** (random /
  majority / minority / all-ones / all-zeros / interleave); the old empirical threshold ran **2–10×
  over ε** (majority attack ≈1.05% — the reproduced Phase-0 over-claim). The extractor now accuses
  only above this `Z`; `argmax` alone is never proof. **Still open (NOT code-level):** media-survival
  certification on **real content / real screen-record / real CMAF lengths**, and the rotation estimator.
- **Chunk 6 — offline forensic extractor.** Landed as the **proven Python reference** under
  `tools/av-forensics/` (offline, operator-run, on already-leaked public files — no key material, not
  in the boundary), re-anchored to the chunk-2 canonical construction. **The load-bearing FM fix is
  preserved:** `fm_register.register` resolves the Fourier-Mellin scale/rotation ambiguity by lowest
  residual **on the valid (non-border) region** — whole-frame residual mis-resolves a crop to the
  wrong scale-direction and sends recovery to garbage. Validated end-to-end (crop+re-encode leak →
  `bitERR 0`, true leaker ranked top with registration; all-erasures / no attribution without). The
  Rust `--extract-av-fingerprint` CLI is **deferred until the scheme is frozen/certified** (avoid a
  blind FFT port of an uncertified scheme — see the chunk-6 decision in `tools/av-forensics/README.md`).

1. ✅ **LANDED (chunk 1).** **Spec + manifest schema.** Define the variant manifest
   (`elastos.ddrm.av-variants/v1`: which segments carry `{A,B}`, the per-variant bit, the codeword
   scheme).
   *Check:* a fixture manifest round-trips through serde and an alignment test asserts
   the schema is referenced by both the mint and serve sides.
2. ✅ **LANDED (chunk 2).** **Codeword derivation.** Pure function `grant → anti-collusion codeword`,
   **timeline-interleaved** and layered over an **erasure-aware code** (§3.4). ECC for
   first light, but **design the schema for tight (Škorić symmetric) Tardos** from the
   start. No I/O.
   *Check (met):* distinct grants → distinct codewords; accusation uses the **analytic
   Tardos threshold** (`canonical.tardos_threshold`, not an empirical `mean+kσ`), validated
   by the **Monte-Carlo FP/FN sweep** (`montecarlo.py`, 2000 trials) against **six collusion
   strategies** (random / majority / minority / all-ones / all-zeros / interleave): FP ≤ ε,
   100% detection. The published per-asset duration bound at this FP-controlled threshold
   still needs **real-media** re-validation (not code-level).
3. **Serve-time selector (behind a flag, single-encode fallback).** `ddrm-media-authority`
   picks `variant[i]` from the codeword when a manifest exists; else serves the single
   encode and sets `fingerprinted:false`.
   *Check:* with a two-variant fixture dir, two different grants produce two different
   served segment-byte sequences; with no manifest, both get the identical single encode.
4. ✅ **LANDED as Python (chunk 6).** **Offline extractor.** Recovers the codeword → buyer,
   mirroring the image extractor. Landed in `tools/av-forensics/` (the proven reference); the Rust
   `--extract-av-fingerprint <file>` CLI is deferred until the scheme is frozen.
   *Check:* a file assembled from a known A/B sequence extracts back to that codeword
   (clean), and still extracts after a lossy re-encode + crop pass (validated: `bitERR 0`, leaker
   ranked top with FM registration).
5. **Mint transcode pipeline (the heavy lift).** Produce `{A,B}` per segment with the
   per-variant DSP (video: spatial/temporal mark; audio: spread-spectrum/echo-hiding),
   CENC-encrypt, publish manifest.
   **Gating DSP research sub-item (the one genuine unknown):** blind **geometric
   registration** of the recovered mark — Phase 0 proved a search loop is insufficient (a
   fine scale grid made detection *worse*); it needs a **deterministic estimator** (an
   embedded template/pilot, or a periodic-carrier **log-polar / Fourier–Mellin** method).
   Until it ships, crop/zoom/rotation are out of envelope (§7). The audio analogue is a
   real **psychoacoustic masking model** for the embed (not the Phase-0 fixed-dB proxy).
   *Check:* a minted asset yields a DASH dir whose A/B segments are perceptually
   indistinguishable (objective metric threshold) yet decode to different bits.
6. **Robustness harness (on real media, not synthetic).** Re-encode / crop /
   screen-record / **bursty segment-loss (erasure)** / **multi-strategy collusion**
   round-trips. **Audio re-validation is mandatory and concrete:** a psychoacoustic
   masking model for the embed; imperceptibility by **PEAQ/ODG plus a human A/B/X
   listening test on real music, speech, and silence**; and **time-stretch / pitch-shift**
   attacks (the audio analogue of geometric desync). Video imperceptibility must be
   re-measured on real content at real resolution (the Phase-0 VMAF figure on synthetic
   480×270 does not count).
   *Check:* documented survival envelope holds **on real media**; the collusion bound is
   certified only after the analytic-threshold + Monte-Carlo + multi-strategy sweep (§5
   chunk 2); out-of-envelope cases are listed honestly (like the image mark's limits).
7. **(Optional, parallel) visible per-session overlay** in the player as an interim,
   clearly-labelled non-forensic deterrent.
   *Check:* overlay shows the buyer's `wallet·time`; documented as deterrent-only.

Phases 1–4 are tractable and self-contained. **Phase 5 is the real cost** (a transcode
pipeline) and gates the rest going live.

---

## 6. Principles conformance (pre-commit self-review hooks)

- **No ambient authority (P3,7,16):** variant choice derives from the *signed grant*,
  server-side; the browser/capsule can't pick or strip its variant.
- **Carrier/provider plane (P2,4,9):** serve-time selection lives in the media-authority
  provider; MSE/DASH/HTTP stay adapters below the contract.
- **One canonical path (P10,11):** the existing quorum-media path gains a selector — no
  hidden unmarked alternate path; absence of variants is *explicit* (`fingerprinted:false`).
- **Fail closed (P11):** missing/partial variant manifests don't silently downgrade to a
  clean copy claimed as marked; the descriptor states the truth.
- **Small trusted core (P5,13):** transcode/DSP and codeword logic live in capsules
  (mint pipeline + media-authority), not the runtime; the runtime only routes the
  capability.
- **No contract drift (P12):** schema, mint, serve, extractor, and these docs land
  together.

## 7. Non-goals / honest limits

- Not EME/Widevine/hardware DRM — we stay at the browser-MSE ceiling on purpose.
- Not protection against a **boundary break** (CEK exfiltration) — that's the dKMS
  threat model.
- **Geometric/temporal desync is out of envelope until registration ships:**
  crop/zoom/rotation (video) and time-stretch/pitch-shift (audio). Phase 0 proved bounded
  translation recovers but scale/zoom needs a deterministic estimator (§5 chunk 5).
- **No certified collusion bound on short clips** — too few marks; short clips are
  single-leaker / c=2 at best, and even that only after FP-controlled certification. Each
  asset's certified `c` is stated in its descriptor; never imply a stronger bound.
- **Phase-0 numbers are synthetic/research-grade.** Audio in particular is *unvalidated on
  real content* (pink-noise test signal + a nominal-dB imperceptibility proxy) until
  chunk 6; the VMAF figure is on synthetic 480×270 and is not the imperceptibility gate.
- The interim visible overlay is **not** forensic and must never be described as such.
- Until Phase 5 ships, AV remains **key-protected, not fingerprinted**, and the UI/docs
  must say so.

## 8. Decisions (resolved in Phase 0) and remaining open items

**Resolved (Phase 0 → bake into the Phase-1/2 schema):**

- **Fingerprint code:** ECC for first light, **tighten to tight (Škorić symmetric)
  Tardos** for the certified scheme; design the schema for Tardos from the start.
- **Variant arity / q-ary is the density lever:** A/B is the baseline; **q-ary
  (A/B/C/D)** packs more codeword bits per segment, roughly *halving* the timeline needed
  for a given collusion order (≈10 min vs ≈20 min for c=3) at higher storage/DSP cost.
- **The collusion bound is per-asset and published** — a function of **duration × mark
  cadence × variant arity**, computed at the **FP-controlled analytic-Tardos threshold**.
  (Phase 0 showed an empirical `mean+3.5σ` threshold is ~1.25% false-accusation — not
  certifiable; the analytic threshold + Monte-Carlo sweep is required, and it pushes the
  duration minimums up.) Long-form supports c=3; short clips get single-leaker / c=2 at
  best. The asset descriptor states its certified `c`.
- **Channel coding is required, not optional:** timeline interleaving + an erasure-aware
  code (§3.4), because the leak channel is bursty (whole-segment loss), not i.i.d.
- **Codeword construction is canonical and RNG-free (landed, chunk 2):** a domain-separated
  SHA-256 stream over integers (quantized arcsine biases + per-grant rows + interleave), so the
  Rust serve selector and the Python extractor derive identical codewords — golden-vector welded
  on both sides. The Phase-0 numpy-`default_rng` derivation is replaced by this.
- **The offline extractor lands as the proven Python reference (chunk 6)**, not a blind Rust port,
  because it is offline/non-boundary and the scheme is not yet certified; the Rust CLI port waits
  for the frozen scheme.

**Still open (resolve in the Phase-1/2 plans):**

- Storage cost of variant encodes — all segments vs. a marked subset (cost vs. codeword
  length/robustness), and how q-ary arity trades against storage.
- Audio: A/B variant (recommended) vs. any in-boundary case for short non-streamed clips.
- The **geometric-registration estimator** choice (embedded template/pilot vs.
  log-polar/Fourier–Mellin) — the one genuine DSP research item (§5 chunk 5).
- Whether the visible interim overlay ships at all, given it can imply false assurance.
