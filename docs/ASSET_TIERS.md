# Asset Tiers — the containment model for a secure digital-asset marketplace

> Status: living map. This is the **product taxonomy** for "an Amazon for digital assets" on
> the ElastOS runtime: every sellable digital good, the *one* way it is contained at playback,
> what works today, the blind spots we can fill quickly, and the North stars.
>
> It complements (does not replace): [PROTECTED_CONTENT.md](PROTECTED_CONTENT.md) (the dDRM
> open path), [DECRYPT_PROVIDER.md](DECRYPT_PROVIDER.md) (the boundary), [CARRIER.md](CARRIER.md)
> (the capability plane), and the contract in [../PRINCIPLES.md](../PRINCIPLES.md).

## 0. The objective

A decentralized marketplace where **any** digital asset is:
1. **packaged once** — encrypted, content-addressed, the CEK split + escrowed to the dKMS quorum;
2. **owned & traded** as tokenized rights on-chain, with built-in royalties; and
3. **played back through a runtime containment boundary** that releases the *experience*
   (pixels, frames, execution) to the owner — never the plaintext or the key — on every device
   and browser.

"Amazon for digital assets" = a universal catalog whose unit of commerce is a DRM-protected,
royalty-bearing, tradable digital good.

## 1. The key insight: there are only five containment strategies

You are not building N viewers. Every asset maps to one of **five** ways to release the
experience without releasing the bytes. Pick the tier first; the file format is a detail.

| Tier | How it protects | Asset types | Status |
|------|-----------------|-------------|--------|
| **1. Stream-decrypt** | CENC, decrypt per-segment **in-boundary**, feed MSE; CEK never leaves | video, audio | ✅ shipped (DASH/ffmpeg + `elacity-player`) |
| **2. Pixel-lock (rasterize)** | render server-side → flattened, **watermarked image**; ship pixels, never source | PDF, images, comics, **text/code**, **SVG**, EPUB, office docs | ✅ PDF, images, comics, text/code, SVG · ⏳ EPUB, office |
| **3. Sandboxed-execute** | run the decrypted bundle **inside a contained runtime**; source never exposed | apps, games, plugins, notebooks, AI models | 🔭 North star (the capsule moat) |
| **4. Entitlement-only** | no content render; a verifiable on-chain **right** | tickets, licenses, credentials, memberships, keys | ⏳ small build (needs a schema decision) |
| **5. Contained-native viewer** | a bundled in-capsule renderer, **no source egress** | 3D (WebGL), fonts | ⚠️ placeholder today |

Why this matters: Tiers 1, 2, and 4 together already cover **all of media + documents +
entitlements** — that is an Amazon-for-media. Tier 3 (apps/games via the runtime sandbox) is
the differentiator competitors structurally cannot copy, because it *is* the capsule model.

## 2. What works today (code-grounded)

### Packaging (mint) — two rails
- **Media rail** (`creator.rs::run_prepare_mint_media`): `video/*` and `audio/*` are packaged
  into multi-bitrate fragmented **DASH/CENC** via `media-provider` (ffmpeg `libx264` + `aac`;
  the audio-only path skips the video filter). Per-track CENC under one asset CEK.
- **Object rail** (`creator.rs::run_prepare_mint`): everything else is sealed as **one inline
  encrypted object**, content-type agnostic, escrowed to the quorum (`quorum_openable`).

So *packaging already accepts any bytes.* The differentiation is all at playback.

### Playback — by tier
| Asset | Today | Tier |
|-------|-------|------|
| `video/*` | `elacity-player` (MSE/DASH) | 1 ✅ |
| `audio/*` | `elacity-player` (single SourceBuffer) — **routing fixed** so audio no longer falls to the object viewer | 1 ✅ |
| `application/pdf` | `ddrm-viewer` pixel-lock pager | 2 ✅ |
| `image/*` (raster) | pixel-lock single page (watermark + EXIF strip) | 2 ✅ |
| `application/x-cbz`, `…comicbook+zip` | pixel-lock pager (natural page order) | 2 ✅ |
| `text/*`, code mimes | pixel-lock rasterised text (anti-copy + watermark) | 2 ✅ |
| `image/svg+xml` | pixel-lock rasterised (no raw scriptable XML) | 2 ✅ |
| 3D / `application/octet-stream` | honest placeholder | 5 ⚠️ |
| EPUB, office docs | not yet (object rail packages them; no secure view) | 2 ⏳ |
| tickets / licenses | not yet | 4 ⏳ |

The pixel-lock set is one predicate, `render::is_pixel_lock`, mirrored by the helper
(`ddrm-media-authority/src/quorum.rs`); the boundary parses once per session (warm
`RenderSession`), caches rendered pages (LRU), and the viewer prefetches neighbours so paging
is instant. The CEK is reconstructed in `Zeroizing` in-VM and **never serialized** into any
response — only `rendered_b64`/`segment_b64` egress.

## 3. Blind spots we can fill quickly (the fast-follows)

| Gap | Risk if left | Fix | Effort |
|-----|--------------|-----|--------|
| ~~Audio routed to object viewer~~ | ~~audio wouldn't play~~ | **DONE** — `is_media_mime` includes `audio/*` | — |
| ~~Text/code shipped raw~~ | ~~copyable, no watermark~~ | **DONE** — Tier-2 rasterise | — |
| ~~SVG shipped raw~~ | ~~scriptable XML + unwatermarked~~ | **DONE** — Tier-2 rasterise via `resvg` | — |
| **Office docs** (docx/pptx/xlsx) | huge catalog category missing | convert → PDF → Tier 2 (sandboxed headless converter — must respect "no ambient authority": run the converter as an explicit, capability-scoped provider, never a bare shell-out) | M |
| **Entitlement-only assets** | can't sell tickets/licenses/memberships | Tier 4: a mint "kind" + a signed-right manifest + a receipt view (needs a schema decision; see §5) | S–M |
| **EPUB** | ebooks unsupported | Tier 2 with HTML/CSS layout (the only Tier-2 type that needs a layout engine) | L |

## 4. North stars (the big, defensible bets)

1. **Sandboxed-execute for apps & games (Tier 3).** "Buy a game; it runs inside a capsule; the
   binary never leaves protected space." No incumbent DRM does decentralized execute-containment;
   this *is* the runtime/capsule thesis. Depends on: a metered, capability-scoped execution
   capsule + a decrypt→execute handoff that never writes plaintext to a shared surface.
2. **3D secure viewer (Tier 5).** Bundled in-capsule WebGL renderer, no source-mesh egress
   (leapfrogs PC2, which ships raw GLB). Net-policy: the renderer ships *with* the capsule (no CDN).
3. **AI models & datasets — decrypt-to-inference inside a TEE.** Ties directly to
   [CONFIDENTIAL_COMPUTE.md](CONFIDENTIAL_COMPUTE.md): the buyer runs the model, never sees the
   weights. The highest-value Tier-3 specialization.
4. **Office docs & EPUB.** Catalog completeness for the "documents" vertical.

## 5. Open product decision — entitlement-only assets (Tier 4)

Tier 4 is small to build but needs a **schema decision** before code, precisely to avoid a
hidden alternate path (Principle 10) and contract drift (Principle 12). The right shape is a
signed manifest that the viewer renders as a *receipt*, not content:
- a mint "kind" (`entitlement`) that seals a small JSON right rather than a media object,
- the on-chain token already proves ownership; the viewer shows the right + provenance,
- no rasterise/stream — there is no content to contain, only a claim to verify.

Deferred until the schema is chosen with product, rather than shipped speculatively.

## 6. 10/10 task specs (for whoever picks these up)

> Framing for the implementer: you are the content-security + media-packaging lead. Each task
> must (a) keep render logic in `decrypt-provider`, never the trusted core (Principle 5/13);
> (b) never let plaintext or the CEK egress (only rendered pixels / decrypted segments);
> (c) fail closed (Principle 11); (d) keep `render::is_pixel_lock` and the helper mirror in sync
> (Principle 12); (e) route through the Carrier-style capability plane, not new host routes
> (Principle 4); (f) add fail-closed tests.

- **Office docs (Tier 2).** Add a capability-scoped document-conversion provider (headless
  LibreOffice/equivalent) that converts docx/pptx/xlsx → PDF, then reuse the existing PDF
  pixel-lock pager. The converter is an explicit provider with a narrow capability — never an
  ambient shell-out from the boundary. Fail closed on conversion error; cap input size.
- **EPUB (Tier 2).** Parse the EPUB (ZIP of XHTML+CSS), paginate via a pure-Rust HTML/CSS layout
  step, rasterise each page like PDF. Reuse the `cbz` archive reader and the page pager. The
  layout engine is the hard, isolated dependency; prove it on a fixed-layout EPUB first.
- **Entitlement-only (Tier 4).** Define the signed-right manifest schema with product; add an
  `entitlement` mint kind; render a verifiable receipt view. No content containment path.
- **3D secure viewer (Tier 5).** Bundle a WebGL renderer *in* the viewer capsule (no CDN, per
  net policy); decrypt mesh/material in-boundary and feed the renderer without exposing the
  source file. Watermark the rendered frames.
- **Sandboxed-execute (Tier 3).** Specify the metered execution capsule and the decrypt→execute
  handoff (plaintext binary never touches a shared surface; execution is capability-scoped,
  audited, and revocable). The flagship; design before code.

## 7. How this conforms to the contract

- **Tiering, not transport, decides containment** — identity/rights stay rooted; HTTP/MSE/image
  routes are adapters below the capsule contract (Principles 2, 4, 9).
- **One canonical path per mime** — `is_pixel_lock` is the single switch; raw `/bytes` is refused
  for pixel-lock mimes; unready tiers fail closed, not half-rendered (Principles 10, 11).
- **The trusted core stays small** — all render code lives in `decrypt-provider`; the core routes
  bytes (Principles 5, 13).
- **Docs ↔ code ↔ tests agree** — this map, `PROTECTED_CONTENT.md`, the `is_pixel_lock` mirror,
  and the per-renderer fail-closed tests move together (Principle 12).
