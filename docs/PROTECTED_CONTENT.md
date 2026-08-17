# Protected Content Provider

Protected content is Runtime-mediated. App, viewer, and content capsules ask to
open an object; they do not receive raw wallet, chain, IPFS, Elacity, or key
authority.

The contract is:

`capsule -> runtime capability -> elastos://drm/open -> drm-provider -> rights/key/decrypt providers`

## Current Slice

The repo now has the contract and fail-closed boundary, not production DRM:

- shared protected-content schemas in `elastos-common`
- `drm-provider` registered as `elastos://drm/*`
- `rights-provider` registered as `elastos://rights/*`
- `key-provider` registered as `elastos://key/*`
- `decrypt-provider` registered as `elastos://decrypt/*`
- `status` advertises the blocked raw-authority list and canonical open
  sequence, including Runtime-owned receipt and audit steps
- `open` validates sealed-object requests and fails closed with the same
  machine-readable required sequence until rights, key, and decrypt providers
  exist
- `open` rejects key envelopes without approved algorithm metadata
- `rights-provider` validates typed access/subscription questions and fails
  closed until a dDRM/chain policy backend is configured
- `chain-provider` exposes typed `has_access_by_content_id` reads that validate
  inputs and only call configured contract selectors
- `key-provider` validates key-release requests and algorithm-agile key
  envelopes, then fails closed until a dKMS backend is configured
- `decrypt-provider` validates scoped decrypt/render session requests and fails
  closed until decrypt/render backends are configured
- `content-provider` rejects incomplete `sealed` object publishes before IPFS:
  `sealed.json`, payload, rights policy, availability receipt, provenance, and
  approved key-envelope algorithms are required

This is intentional. The first safe step is to make the authority boundary
unambiguous before adding dDRM contract reads or ElastOS dKMS.

PC2's dDRM contracts and WASM decrypt/render/media crates are useful
implementation references. They should enter Runtime only as provider-internal
backends behind `rights-provider`, `key-provider`, and `decrypt-provider`; they
must not give app or viewer capsules raw CEK, wallet, chain, IPFS, or Elacity
authority.

## dDRM Decrypt Rail Options And Recommendation

The v0.4.0 provider chain intentionally proves the fail-closed sockets:

`drm-provider -> rights-provider -> key-provider -> decrypt-provider`

The remaining architecture decision is how the live CEK reaches the decrypt
boundary once real decryption is wired. The recommended default is a
sealed-material rail:

- Runtime orchestrates the normal provider chain through `drm`, `rights`, and
  `key`.
- `decrypt-provider` creates a per-session one-time public key for the decrypt
  sandbox.
- `key-provider` or the dKMS release backend seals the CEK to that one-time
  decrypt-session public key, using the approved PQ-hybrid envelope profile.
- The decrypt step receives sealed material in the decrypt-session request,
  unwraps it inside the sandbox, decrypts/renders, zeroizes the live CEK, and
  returns only scoped output.
- `decrypt-provider` does not pull keys by making outbound capability calls.
  The component that briefly sees the live CEK must have the smallest possible
  authority surface.

This keeps the rights/key/decrypt separation while avoiding an outbound
authority grant to the highest-risk boundary. It also explains the current
schema gap: `ReleaseReceiptV1` proves authorization, but it intentionally carries
no key material; the next contract addition should add a sealed decrypt material
envelope to the key/decrypt handoff instead of sending a raw CEK or letting
decrypt fetch one.

Other options were considered and rejected as the normal Runtime path:

| Option | Shape | Assessment |
|--------|-------|------------|
| Decrypt pulls keys | `decrypt-provider` calls `key-provider` or a key backend after authorization | Flexible, but grants outbound authority to the boundary that briefly holds live CEK. Keep only for controlled diagnostics or explicit capability-gated adapters. |
| One combined key/decrypt provider | Key release and decrypt/render run in one provider | Simpler CEK path, but collapses authority separation and increases blast radius. Useful for tests, not the target trust boundary. |
| Runtime relays raw CEK | Runtime receives CEK and passes it to decrypt | Not acceptable. Runtime would become a key-material holder instead of an orchestrator. |
| Runtime relays sealed material | Runtime passes a sealed envelope without being able to open it | Acceptable if the envelope is transcript-bound and Runtime never sees raw CEK. This is the practical form of the recommended rail. |
| dKMS direct-to-decrypt sealing | dKMS seals directly to the one-time decrypt-session key | Best target when available. `key-provider` brokers policy and receipts without seeing raw CEK. |
| Lit/Chipotle backend | Vendor-backed key release returns a CEK envelope, as PC2 does today | Useful compatibility backend only. The Runtime contract must remain backend-neutral and must also support an ElastOS-native dKMS path. |

Before live decrypt is enabled, the sealed material envelope must bind the full
transcript: principal, session, object, action, viewer interface, output kind,
expiry, release receipt hash, decrypt-session public key, envelope algorithm,
and provider identity. It must use nonce-safe authenticated encryption, signature
verification, replay rejection, short expiry, zeroization, and audit. PC2's
current `ddrm-decrypt` WASM pattern proves the containment invariant, but its
P-256 and Lit/Chipotle details are implementation references rather than
Runtime product truth.

## Protected Object Shape

New protected objects should publish as sealed SmartWeb objects:

```json
{
  "schema": "elastos.sealed.object/v1",
  "payload_cid": "bafy...",
  "rights_policy_cid": "bafy...",
  "availability_receipt_cid": "bafy...",
  "key_envelope": {
    "scheme": "elastos-pq-hybrid-threshold-v0",
    "kid": "...",
    "wrapped_cek": "...",
    "policy_hash": "sha256:...",
    "algorithms": {
      "cipher": "aes-256-gcm",
      "signature": ["ed25519", "ml-dsa-65"],
      "kem": ["x25519", "ml-kem-768"],
      "share_scheme": "shamir-t-of-n"
    }
  },
  "viewer": {
    "required_interface": "elastos.viewer/document@1"
  }
}
```

`payload_cid` can be publicly reachable because protected payload bytes must be
encrypted before replication. Access is enforced by rights checks and key release,
not by hiding CIDs.

## Crypto Agility And dKMS Direction

FROST is a threshold Schnorr protocol, so it is classical ECC security, not a
post-quantum root. ElastOS may use FROST for short/medium-term receipt or cohort
signing, but new dKMS content must not depend on FROST as the long-term key
security foundation.

New protected content should use algorithm-agile sealed objects:

- Encrypt payload bytes with AES-256-GCM or ChaCha20-Poly1305.
- Split the AES-256 CEK into `t-of-n` shares.
- Wrap each share to an approved dKMS node with hybrid X25519 + ML-KEM-768.
- Sign release receipts with classical + PQ signatures where practical, starting
  with Ed25519 plus ML-DSA; use SLH-DSA for conservative hash-based signatures
  where size and speed are acceptable.
- Reconstruct the CEK only inside the key/decrypt provider boundary, then return
  scoped render/decrypt output to the viewer instead of raw CEKs.
- When the decrypt engine is wired, prefer dKMS-direct sealing to the decrypt
  session key. If an intermediate key-provider re-seal is used during migration,
  it must remain provider-internal, signed, auditable, and short-lived.

Current EVM/BTC/ELA wallet proofs and dDRM chain state are still classical. They
are useful authorization inputs today, but they should not be the only permanent
identity or access root for long-lived encrypted assets.

References: [NIST PQC standards announcement](https://www.nist.gov/news-events/news/2024/08/nist-releases-first-3-finalized-post-quantum-encryption-standards),
[FIPS 203 ML-KEM](https://csrc.nist.gov/pubs/fips/203/final),
[FIPS 204 ML-DSA](https://csrc.nist.gov/pubs/fips/204/final),
[FIPS 205 SLH-DSA](https://csrc.nist.gov/pubs/fips/205/final),
and [RFC 9591 FROST](https://www.rfc-editor.org/rfc/rfc9591).

## Provider Boundary

Normal capsules must not see:

- raw CEKs
- wallet RPC or private keys
- arbitrary chain RPC
- Kubo/IPFS APIs
- Elacity SDK or pinning credentials

The provider plane should expose typed questions instead:

- `elastos://drm/open`
- `elastos://rights/access/has_access_by_content_id`
- `elastos://rights/subscription/is_subscription_active`
- `elastos://rights/content/can_stream`
- `elastos://rights/content/can_download`
- `elastos://chain/<network>/rights/has_access_by_content_id`
- `elastos://key/release`
- `elastos://decrypt/session/open`
- `elastos://decrypt/render`

## Pixel-Lock Secure Rendering (documents)

Some content types must never reach the browser as their source file — a PDF handed to
the browser is a raw, re-extractable, unwatermarked copy (and browsers like Brave block
`blob:`/`data:` PDFs in iframes anyway). For these, the runtime uses a **pixel-lock**
tier (PC2 `ddrm-renderer` parity): the asset is rasterised to flattened, buyer-watermarked
page **images** *inside the decrypt boundary*, and only those images egress.

- Renderer placement: the `decrypt-provider` capsule (feature `pdf-render`), NOT the
  trusted core. It uses pure-Rust rasterisers — `hayro` (PDF, `#![forbid(unsafe_code)]`),
  `resvg`/`tiny-skia` (SVG), `image` (raster + JPEG encode), `zip` (CBZ archive), and
  `ab_glyph` + vendored DejaVu fonts (anti-aliased body text — proportional for prose, mono
  for code — AND the faint tiled forensic watermark, same vector face). The trusted core gains
  no render code — it only routes bytes (Principle 5/13).
- Containment: the `StreamSegment` op takes an optional `render` directive; when present
  for a pixel-lock mime, the boundary extracts the object from the decrypted fragment and
  returns a watermarked JPEG (`rendered_b64` + `total_pages`) — never the raw bytes. The
  recovered CEK stays in-VM and the plaintext document never leaves the sandbox.
- Fail-closed: a non-pixel-lock mime, an unparseable object, or a render error returns an
  error with no bytes (Principle 11). There is **one canonical path** per mime — the raw
  `/bytes` egress is refused for pixel-lock sessions (Principle 10).
- Resource bounds (bound-before-you-allocate): a creator controls the source file, so every
  raster decode and rasterisation is bounded BEFORE allocating, to stop a tiny crafted "pixel
  bomb" from forcing a multi-GB allocation in the boundary. Raster decode (single image + CBZ
  pages) runs through `decode_bounded` with strict `image::Limits` (max dims `MAX_DIM`, max
  alloc `MAX_DECODE_BYTES`); the PDF rasteriser bounds its scale on both axes and by area
  (`MAX_PIXELS`) with a final predicted-size guard; CBZ caps each page and the archive total
  (which also bounds warm session memory). Not yet bounded: per-format decode *time* (a
  CPU-pathological but small file) — a wall-clock/watchdog follow-up, called out, not assumed.
- Browser contract (served by `elastos-server`, routing only):
  - `GET /api/viewers/ddrm-viewer/object/{session}` → manifest adds
    `pixel_locked: true`, `total_pages`, `page_content_type`.
  - `GET /api/viewers/ddrm-viewer/object/{session}/page?n=N` → one rendered page (content type =
    the manifest's `page_content_type`: `image/jpeg` for pixel-lock, `text/html` for an EPUB
    chapter) + `X-Asset-Pages`/`X-Asset-Page` headers.
  - `GET …/object/{session}/bytes` → `403` for render-locked (pixel-lock + html-lock) sessions;
    decrypt-passthrough assets (3D `model/*`) are served here.
  - `POST …/object/{session}/close` (and the media twin `…/media/{session}/close`) →
    explicit session release, same authorization gate as the reads (launch token +
    viewer + principal). Idempotent and non-leaking: closed / already-gone / not-owned
    all answer `204`, so session ids cannot be probed; a close can only cost a re-open.
- Session lifecycle (the release half of the open pipeline — `api/session_lifecycle.rs`):
  a viewer session pins the gateway-spawned authority subprocess chain, so release has
  three independent paths. (1) Explicit close: the viewer capsules fire the close route
  on `pagehide` (`fetch` keepalive — it carries the token header; `sendBeacon` cannot)
  and the Home shell releases the session of any viewer window it closes. (2) A 60s
  registry sweeper releases expired sessions on the clock, so an idle machine cannot
  keep an expired authority subprocess alive past its TTL. (3) The store's own lazy
  sweep on lookup/admission remains as the backstop. New session kinds register with
  the `SessionLifecycle` trait; the routes and sweeper never change. Design:
  `docs/superpowers/specs/2026-07-22-session-lifecycle-design.md`.
- Watermark (two layers, both carrying the SAME identity — `wallet · content · time`, where
  `wallet` is the FULL owner EVM address recovered from the wallet-signed access grant, `content`
  the short on-chain content id, and `time` the UTC open-minute):
  1. **Visible**: stamped diagonally + tiled across every page (forensic provenance, uncroppable),
     rendered in the anti-aliased DejaVu mono face at low opacity (a quiet caption, not the old
     blocky bitmap).
  2. **Invisible** (`render/invisible.rs`): a blind **DCT-domain QIM** stamp embedded in luminance
     under perceptual masking (flat white margins left pristine). Each 8×8 block carries one bit in
     the parity of `round((A−B)/STEP)` for two symmetric mid-band coefficients `A=(1,2)`, `B=(2,1)`;
     QIM (vs a fixed margin) bounds the per-block nudge so a high-contrast block can never end up
     carrying the wrong bit. To keep the codeword recoverable from CONTENT-SPARSE pages (a short
     snippet on a mostly-empty page), the invisible layer carries a COMPACT, self-describing payload
     framed as `SYNC|LEN|DATA|CRC-16` (232-bit period), raster-tiled and majority-vote folded. On a
     production (grant-bearing) open `DATA` is the **authenticated anchor** `[wallet_prefix(4) +
     grant_digest(16)]` (21 B ≤ the 24-byte cap, so the period is unchanged); on a no-grant/local-dev
     open it falls back to the compact 20-byte wallet. Either way a LEAKED frame stays attributable
     even if the visible mark is cropped/painted out — but it is a **tracer with non-repudiation, not
     tamper-proof evidence**, and (production case) the invisible layer no longer reveals the full
     wallet — only a digest that *commits to* it (see *Forensic strength & privacy* below).
     Recovers through our q85 encode, moderate recompression, brightness/contrast shifts,
     same-resolution screenshots, and vertical offsets; does NOT survive rescaling/rotation/
     width-changing crops (a geometric-sync layer is future work). NB "same-resolution screenshot"
     means a 1:1 pixel-grid capture — a HiDPI/Retina screenshot resamples (≈2× upscale), which is
     rescaling, so it falls under the unsupported case and generally will NOT recover; most
     real-world screenshots on HiDPI displays are out of envelope. Validated end-to-end against the
     actual rendered text AND (sparse) code pages, not just synthetic images. Offline read:
     `decrypt-provider --extract-watermark <image> [--verify-grant <grant.json>]` — prints the wallet
     prefix + grant digest (or the `0x…` wallet for a legacy compact mark) and, given a candidate
     grant, reports MATCH / NO MATCH by recomputing via the shared `grant_watermark_digest16`.
- **Fail closed**: the single pixel-lock egress (`watermark::finalize`) and the HTML-lock egress
  (EPUB) both REFUSE to emit a protected page without a non-empty forensic stamp — no identity, no
  image. There is no code path that emits protected pixels without a traceable mark.
- **Forensic strength & privacy (honest scope — state it exactly, like the threat model does).** Two
  things this watermark is deliberately NOT:
  1. **Authenticated (non-repudiable), but not yet unforgeable by a third party.** The invisible
     layer embeds an **authenticated grant digest** — `ddrm_envelope::grant_watermark_digest16` =
     SHA-256 of the buyer's wallet-signed delegation signature, truncated to 16 bytes — alongside a
     4-byte wallet prefix (a 21-byte payload, ≤ the 24-byte cap, so sparse-page recovery is
     unchanged). This gives **non-repudiation** — only the buyer's own wallet signature reproduces
     that digest — and raises forgery from *"anyone can plant any wallet"* to *"only a party holding
     the victim's signed grant can."* It is **not** the full anti-framing guarantee: the delegation
     signature is not a hard secret (it transits the recover flow), so whoever obtains a victim's
     grant could still replant it; the frame `SYNC|LEN|DATA|CRC` still uses a CRC for frame
     *integrity*, not authenticity. Treat the mark as a **strong tracer with non-repudiation**, not
     yet court-grade against a determined framer. North-star upgrade: a **server-key MAC**, or an
     **opaque token resolved via the custody log**, so no identity-derived value rides in the mark at
     all. The authenticated, tamper-evident system-of-record remains the hash-chained, signed
     **custody audit log** ([THREAT_MODEL.md](THREAT_MODEL.md) §4), which also retains the grant
     digest (minimized — option C) so a leaked frame can be *verified* against a candidate grant.
  2. **Not anonymous (the visible layer).** The **visible** stamp embeds the **full** opening wallet
     (human-readable); the **invisible** layer carries only the 4-byte prefix + grant digest, not the
     full wallet. So anyone who sees a rendered page — a screenshot, a screen-share, a photo, a leaked
     frame — recovers the buyer's on-chain identity **from the visible mark**. This is the deliberate
     trade: leak-attribution in exchange for buyer anonymity at view time
     ([THREAT_MODEL.md](THREAT_MODEL.md) §3, §6).

Pixel-lock covers, per renderer in `decrypt-provider/src/render`:
- `application/pdf` → `pdf` (multi-page, `hayro`)
- `application/vnd.comicbook+zip`, `application/x-cbz` → `cbz` (multi-page, natural page order)
- `text/plain`, `text/markdown` (prose) → `text` (multi-page **light reading reader**: a real
  anti-aliased PROPORTIONAL vector face (DejaVu Sans via `ab_glyph`) on a warm page with line
  leading, true measured-width word-wrap and full Unicode — smart quotes/dashes/accents render as
  themselves — rasterised so source can't be copied)
- source-code mimes (`application/json`, `application/javascript`, `application/xml`,
  `application/x-yaml`, `application/toml`, `application/x-sh`) → `code` (multi-page **dark code
  view**: anti-aliased fixed-pitch face (DejaVu Sans Mono via `ab_glyph`) with a line-number gutter
  + conservative per-language colour for comments/strings/numbers/XML tags, base16-ocean theme —
  mirrors PC2 `render::code` intent)
- `image/svg+xml` → `svg` (rasterised to pixels — SVG is scriptable XML, never shipped raw)
- other `image/*` → `image_page` (single page, re-encoded so source file + EXIF is stripped)

A second render-lock variant, **html-lock**, serves reflowable EPUB without rasterising:
- `application/epub+zip`, `application/epub` → `epub` (one "page" per spine chapter). The boundary
  reads the EPUB ZIP, resolves the OPF spine order (`roxmltree`), and for each chapter emits a
  **sanitised, self-contained HTML document** (scripts/styles/handlers/dangerous tags stripped,
  `javascript:` URLs neutralised, images inlined as `data:` URIs, our reader CSS + a tiled
  forensic watermark + `user-select:none` + a strict CSP `<meta>`). The page is served with
  `page_content_type: text/html; charset=utf-8` **and an enforced HTTP `Content-Security-Policy`
  response header carrying a `sandbox` directive** (`default-src 'none'; … ; sandbox`), plus
  `X-Content-Type-Options: nosniff` — so the document is sandboxed **at the resource level by the
  browser even if loaded directly or framed without the attribute** (no script, no same-origin, no
  network, no form post). The viewer additionally renders it in a `sandbox=""` iframe. The
  containment order is now: **enforced HTTP CSP `sandbox` (true layer) ▸ inline `<meta>` CSP +
  iframe attribute (belt) ▸ the hand-rolled sanitiser (defence-in-depth)** — the sanitiser is no
  longer the sole barrier. The raw EPUB ZIP never egresses (`/bytes` is refused like any
  render-lock asset). This mirrors PC2's `EpubRenderer` html-lock tier rather than a pixel-lock
  rasterise.

The single source of truth for the render-locked set is `render::is_pixel_lock` (covers pixel-lock
**and** html-lock mimes); the media-authority helper (`scripts/dev/ddrm-media-authority/src/quorum.rs`)
mirrors it and the two must stay in sync (Principle 12). The `page_content_type` is decided by the
renderer (`RenderSession::page_content_type` → `image/jpeg` or `text/html`) and threaded through the
helper descriptor to the viewer. Office docs remain tracked in `docs/ASSET_TIERS.md`.

3D (`model/*`: glTF/GLB/STL/OBJ) is NOT render-locked — it is **decrypt-passthrough** (Tier 5):
the boundary decrypts the mesh and serves the cleartext bytes via `/bytes`, which the `ddrm-viewer`
renders with a **bundled, local Three.js** WebGL viewer (no CDN, vendored under the capsule's
`/vendor`). Like PC2's 3D path, the cleartext mesh reaches the browser; frames are not watermarked
yet (see `docs/ASSET_TIERS.md` North star 2 for the render-only-containment upgrade).

Audio/video are NOT pixel-lock — they take the stream-decrypt rail (DASH/CENC + MSE);
`viewer_open::is_media_mime` routes `video/*` and `audio/*` to the media player. A minted dDRM
media `.ddrm` opens via the **quorum-media** path (`viewer_open::open_quorum_media`): the gateway
fetches the published DASH directory by its `asset_cid`, then `ddrm-media-authority --quorum
--dash-dir` recovers the CEK 2-of-3 and serves CENC-decrypted fragments per-segment over the same
descriptor/`segment` protocol as the local media path. The whole ordered segment set is welded
into the transcript AAD (`to_aad_with_segments`), so a substituted/reordered fragment fails the
CEK unwrap closed before any byte is decrypted; the recovered CEK never leaves the decrypt VM.

AV is therefore **key-protected, not yet fingerprinted**: the decrypted segment bytes that reach
MSE carry no per-buyer mark (the browser-MSE ceiling without EME/Widevine — exactly PC2's model).
The forensic upgrade (A/B variant watermarking for video, spread-spectrum/echo-hiding for audio,
chosen per buyer from their signed grant at serve time) is a transcode-pipeline roadmap item, not a
patch — see [AV_WATERMARKING.md](AV_WATERMARKING.md). Until it ships, UI/docs must not claim AV is
watermarked.

## Remaining Sequence

1. Wire real `elastos://drm/open` orchestration behind the declared sequence:
   content status/fetch, typed rights checks, rights-bound key release,
   release-receipt-bound decrypt/render sessions, sealed decrypt material, and
   signed release receipts.
2. Wire `key-provider` to an ElastOS PQ-hybrid threshold release backend.
3. Wire `decrypt-provider` to a real decrypt/render backend that keeps CEKs
   inside the provider boundary.
4. Wire real protected-content producers to the existing sealed-object publish
   contract after payload encryption, rights policy, availability receipt,
   provenance, key-envelope, and viewer-interface generation exist.
5. Add a permissioned ElastOS PQ-hybrid dKMS v0 for new content only.

Visible protected-content UI may ship only as a disabled/read-only readiness
rail until fail-closed provider tests and capability-resource checks cover the
full open path. The current Library rail can show Provider Chain/status
receipts and a disabled `Encrypted recipients` option, but it must not claim
production encrypted-recipient sharing, dDRM completion, or generic decrypt/
render readiness.

## Executable Proof

Run `scripts/protected-content-provider-contract-smoke.sh` after changing
protected-content provider capsules. It exercises the real provider binaries
over their JSON line protocol and verifies the current journey contract:

- status exposes blocked raw authority
- valid requests fail closed until backends are configured
- invalid raw-authority requests are rejected
- `drm-provider.open` reports the declared provider/runtime sequence
