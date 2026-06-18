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
- Browser contract (served by `elastos-server`, routing only):
  - `GET /api/viewers/ddrm-viewer/object/{session}` → manifest adds
    `pixel_locked: true`, `total_pages`, `page_content_type`.
  - `GET /api/viewers/ddrm-viewer/object/{session}/page?n=N` → one rendered page (content type =
    the manifest's `page_content_type`: `image/jpeg` for pixel-lock, `text/html` for an EPUB
    chapter) + `X-Asset-Pages`/`X-Asset-Page` headers.
  - `GET …/object/{session}/bytes` → `403` for render-locked (pixel-lock + html-lock) sessions;
    decrypt-passthrough assets (3D `model/*`) are served here.
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
     snippet on a mostly-empty page), the invisible layer carries a COMPACT identity — the 20-byte
     owner EVM wallet (the visible mark + audit log keep the full `wallet · content · time` tuple) —
     framed as `SYNC|LEN|DATA|CRC-16` (232-bit period), raster-tiled and majority-vote folded, so a
     LEAKED frame stays attributable to the wallet even if the visible mark is cropped/painted out.
     Recovers through our q85 encode, moderate recompression, brightness/contrast shifts,
     same-resolution screenshots, and vertical offsets; does NOT survive rescaling/rotation/
     width-changing crops (a geometric-sync layer is future work). Validated end-to-end against the
     actual rendered text AND (sparse) code pages, not just synthetic images. Offline read:
     `decrypt-provider --extract-watermark <image>` (prints the recovered `0x…` wallet).
- **Fail closed**: the single pixel-lock egress (`watermark::finalize`) and the HTML-lock egress
  (EPUB) both REFUSE to emit a protected page without a non-empty forensic stamp — no identity, no
  image. There is no code path that emits protected pixels without a traceable mark.

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
  `page_content_type: text/html; charset=utf-8`; the viewer renders it in a **script-less sandbox
  iframe** (`sandbox=""` — no allow-scripts, no allow-same-origin), so a hostile book is fully
  inert. The raw EPUB ZIP never egresses (`/bytes` is refused like any render-lock asset). This
  mirrors PC2's `EpubRenderer` html-lock tier rather than a pixel-lock rasterise.

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

## Remaining Sequence

1. Wire real `elastos://drm/open` orchestration behind the declared sequence:
   content status/fetch, typed rights checks, rights-bound key release,
   release-receipt-bound decrypt/render sessions, and signed release receipts.
2. Wire `key-provider` to an ElastOS PQ-hybrid threshold release backend.
3. Wire `decrypt-provider` to a real decrypt/render backend that keeps CEKs
   inside the provider boundary.
4. Wire real protected-content producers to the existing sealed-object publish
   contract after payload encryption, rights policy, availability receipt,
   provenance, key-envelope, and viewer-interface generation exist.
5. Add a permissioned ElastOS PQ-hybrid dKMS v0 for new content only.

No visible protected-content UI should ship before fail-closed provider tests and
capability-resource checks cover the full open path.

## Executable Proof

Run `scripts/protected-content-provider-contract-smoke.sh` after changing
protected-content provider capsules. It exercises the real provider binaries
over their JSON line protocol and verifies the current journey contract:

- status exposes blocked raw authority
- valid requests fail closed until backends are configured
- invalid raw-authority requests are rejected
- `drm-provider.open` reports the declared provider/runtime sequence
