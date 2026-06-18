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
   mark per variant). Both are CENC-encrypted and published in the DASH directory.
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
  on already-published ciphertext. No on-the-fly re-encryption.
- **One canonical path**: the quorum-media serve path gains a selector; there is no
  second "unmarked fast path." If variants are absent, the path serves the single
  encode and the descriptor says `fingerprinted: false` — explicit, not silent.

---

## 5. Phasing (each chunk has a one-sentence pass/fail check)

> Per [CLAUDE.md](../CLAUDE.md): smallest independently-verifiable steps. These are
> roadmap chunks; each becomes its own approved plan + tests before code.

1. **Spec + manifest schema.** Define the variant manifest (`elastos.ddrm.av-variants/v1`:
   which segments carry `{A,B}`, the per-variant bit, the codeword scheme).
   *Check:* a fixture manifest round-trips through serde and an alignment test asserts
   the schema is referenced by both the mint and serve sides.
2. **Codeword derivation.** Pure function `grant → anti-collusion codeword` (start with
   ECC; design for Tardos). No I/O.
   *Check:* unit tests — distinct grants give distinct codewords; the documented
   collusion bound holds for `n` colluders on synthetic codewords.
3. **Serve-time selector (behind a flag, single-encode fallback).** `ddrm-media-authority`
   picks `variant[i]` from the codeword when a manifest exists; else serves the single
   encode and sets `fingerprinted:false`.
   *Check:* with a two-variant fixture dir, two different grants produce two different
   served segment-byte sequences; with no manifest, both get the identical single encode.
4. **Offline extractor.** `--extract-av-fingerprint <file>` recovers the codeword →
   buyer, mirroring the image extractor.
   *Check:* a file assembled from a known A/B sequence extracts back to that codeword
   (clean), and still extracts after a lossy re-encode pass.
5. **Mint transcode pipeline (the heavy lift).** Produce `{A,B}` per segment with the
   per-variant DSP (video: spatial/temporal mark; audio: spread-spectrum/echo-hiding),
   CENC-encrypt, publish manifest.
   *Check:* a minted asset yields a DASH dir whose A/B segments are perceptually
   indistinguishable (objective metric threshold) yet decode to different bits.
6. **Robustness harness.** Re-encode / crop / screen-record-sim / partial-collusion
   round-trips.
   *Check:* documented survival envelope holds on the harness; out-of-envelope cases are
   listed honestly (like the image mark's stated limits).
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

- Not EME/Widewine/hardware DRM — we stay at the browser-MSE ceiling on purpose.
- Not protection against a **boundary break** (CEK exfiltration) — that's the dKMS
  threat model.
- The interim visible overlay is **not** forensic and must never be described as such.
- Until Phase 5 ships, AV remains **key-protected, not fingerprinted**, and the UI/docs
  must say so.

## 8. Open decisions (resolve in the Phase-1/2 plans)

- Fingerprint code: ECC first vs. Tardos from the start (collusion resistance vs. length).
- Storage cost of 2× segment encodes — all segments vs. a marked subset (cost vs.
  codeword length / robustness).
- Audio: A/B variant (recommended) vs. any in-boundary case for short non-streamed clips.
- Whether the visible interim overlay ships at all, given it can imply false assurance.
