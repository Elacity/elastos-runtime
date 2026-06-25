# PC2 Players & dDRM Decrypt — Architecture Alignment

**Status:** Validated against PC2 `main` + runtime principles. The downstream viewer
seam (B1–B3: `elacity-player` + runtime media routes + decrypt-provider
`stream_segment`) is **built and gate-green**; only the upstream live CEK producer
remains gated on Anders (local KMS / dKMS). See "Viewer seam — implemented" below.
**Date:** 2026-06-08 (viewer seam added 2026-06-11)
**Branch:** `feat/decrypt-provider-cenc`
**Sources:** PC2 `pc2-node/wasm-apps/{ddrm-decrypt,ddrm-renderer}`,
`pc2-node/wasm-renderer`, `pc2-node/crates/*`,
`docs/handover/PC2_CONVERGENCE_INVENTORY_FOR_RUNTIME.md`.
**Cross-refs:** `DDRM_DECRYPT_RAIL.md`, `DDRM_STATUS.md`, `CONVERGENCE_PLAYBOOK.md`.

## Why this doc

We are converging PC2's proven dDRM pipeline (Ross's wasm/rust players + the
decrypt engine) into the ElastOS Runtime capsule model. This validates the player
split and the decrypt boundary against runtime principles **and** against the two
security invariants raised by Irzhy, so the bring-across stays correct by
construction rather than by retrofit.

## The two players (both `viewer` capsules — neither ever sees the CEK)

| Player | PC2 source | Content | Capsule role / type |
|---|---|---|---|
| **Media player** (`elacity-player`) | DASH/CENC + `cenc-decrypt`, `mp4-split` | video, audio (`.mp4`, `.mkv`, audio) | `viewer`, `wasm` |
| **Non-media player** (`ddrm-viewer` / `wasm-renderer`) | `wasm-renderer/render/{pdf,epub,cbz,image,code,text}` + watermark | PDF, EPUB, CBZ, images, source code | `viewer`, `wasm` |

Both are *consumers* of scoped, already-decrypted output. The non-media player
additionally enforces presentation-layer lockdown (pixel-lock for image/PDF/CBZ/
code; html-lock + forensic watermark for reflowable EPUB) — but that is rendering
policy, not key custody.

## Irzhy's security invariants (adopted as binding rules)

1. **Encrypt:** the CEK and KID are generated **inside** the wasm boundary; only
   the ciphertext and its non-secret relatives (KID, IV, metadata) are emitted.
   *(PC2 precedent: `wasm-renderer` `encrypt_only` mode — "the CEK never leaves
   WASM memory".)*
2. **Decrypt:** the CEK is **never** passed as input to any other component in
   plaintext. CEK recovery (from the License/CEK provider envelope) **and**
   content decryption happen **together inside one wasm boundary**, with
   zeroization at the end. *(PC2 precedent: `ddrm-decrypt` — "CEK never crosses
   the FFI boundary"; per-request CEK held in `Zeroizing`.)*

**Governing rule for the runtime:** CEK-recovery and content-decryption MUST be
colocated in the decrypt-provider's sandbox. Splitting them across the FFI/IPC
boundary is forbidden — that is the one move that would expose a live key.

## How this maps onto runtime tiers

```
rights-provider  ──▶ key-provider ──▶ decrypt-provider ──▶ viewer (player) capsule
 (policy/access)     (seals CEK to     (RECOVERS CEK +        (renders scoped
                      decrypt session   DECRYPTS content       output; NO CEK)
                      pubkey via ECDH)   in ONE boundary;
                                         zeroizes)
```

- **decrypt-provider** = PC2 `ddrm-decrypt`. Owns a session keypair, receives the
  ECDH-sealed envelope + ciphertext, unwraps the CEK internally, decrypts, emits
  scoped output. CEK lives only here, in `Zeroizing`. *(Invariant 2.)*
- **key-provider / rights-provider** = the upstream seal/policy. The key-provider
  seals the CEK **to the decrypt session's public key** (ECDH envelope) only after
  the rights-provider returns an `allowed` decision bound to principal/session/
  content/right. The live CEK is never handed around in cleartext.
- **player/viewer capsules** = PC2 `elacity-player` / `ddrm-viewer`. Receive
  `render_only`-style scoped bytes. Never receive `cek`/`iv`. *(Invariant 2.)*
- **(future) encrypt/creator path** = PC2 `cenc-encrypt` / `encrypt_only`.
  Generate CEK+KID inside wasm, emit ciphertext only. *(Invariant 1.)*

## The decrypt rail, made concrete (answers `DDRM_DECRYPT_RAIL.md`)

PC2's scheme is **Option A** with a specific crypto shape: a **P-256 ECDH
envelope**. The upstream sealer derives a shared secret to the decrypt session's
public key, AES-256-CBC-wraps the CEK, and ships an envelope; the decrypt-provider
performs ECDH with its session secret key and unwraps the CEK **in-boundary**.
This keeps the highest-authority component (decrypt) free of outbound network/
capability authority — the safest split, and the one PC2 already runs in
production.

We have captured this as an executable, tested characterization spec at
`capsules/decrypt-provider/src/envelope.rs` (vendored from PC2 `ddrm-decrypt`):

- `parse` → `ecdh_unwrap` → `extract_cek`, all returning `Zeroizing` material.
- Golden round-trips for envelope v0x02 (fixed IV) and v0x03 (random IV).
- Fail-closed on truncated envelopes and on a wrong session key.
- CEK-containment assertion: the raw CEK never appears in the sealed envelope.

It is deliberately **not yet wired** into the live `OpenSession`/`Render`
dispatch — that one-step wiring lands once Anders confirms (a) Option A and (b)
the session-key provisioning path. Contract-first, by design.

## Consumer contract — what each player receives (and never receives)

The downstream boundary (decrypt-provider → player) is fully rail-independent and
is now pinned by characterization tests in `decrypt-provider`. PC2's
`ddrm-decrypt/media.rs` states the rule verbatim: *"Public functions never accept
or return a CEK; only `request_handle: u32`."*

| Player | Receives | Addressed by | Never receives |
|---|---|---|---|
| **Media** (`elacity-player`) | decrypted fMP4 segments (init + media) for MSE `appendBuffer`, streamed per segment | opaque session/request handle | CEK, IV, raw key bytes |
| **Non-media** (`ddrm-viewer`) | `render_only` plaintext / rendered pixels (pixel-lock) or sanitized XHTML + watermark (html-lock) | opaque session id | CEK, IV, raw key bytes |

Runtime enforcement (tests in `capsules/decrypt-provider/src/main.rs`):
- the scoped response carries **metadata only** — an allow-list of
  `schema, session_id, object_cid, viewer_interface, output_kind, is_protected,
  sample_count, expires_at`;
- a forbidden-key check rejects any `cek/iv/key/plaintext/decrypted/secret/...`
  field ever appearing in the player-facing response, for **both** player kinds;
- a media-segment decrypt run asserts neither the CEK nor the decrypted bytes
  reach the scoped (player-facing) output.

This means the two ends of the chain already meet in the middle: the upstream rail
(envelope unwrap, pending Anders) hands the CEK only into the decrypt boundary, and
the downstream boundary provably emits scoped output with the key confined.

## Isolation tier — evidence for the open question

PC2 runs both `ddrm-decrypt` and the players as **`wasm32-wasip1`**, and the
convergence inventory states plainly: *"The Runtime trusted base does not need to
know dDRM exists … dDRM stays in userland where the protocol can evolve."* PC2
also explicitly replaces *iframe sandbox (escapable)* with *WASM/microVM sandbox
(enforced)*.

This supports running the dDRM providers as **`type: wasm`** (wasmtime sandbox)
rather than `type: microvm`, given runtime is wasm-first today and the wasm
boundary already delivers the required containment (CEK never crosses FFI +
`Zeroizing`). Our `decrypt-provider/capsule.json` currently declares
`type: microvm`; aligning it to `wasm` is part of the Anders tier decision we have
queued — not changed unilaterally here.

## Bring-across order (contract-first, one boundary at a time)

1. **decrypt rail wiring** (after Anders confirms): compose `envelope::ecdh_unwrap`
   + `cenc::process` inside decrypt-provider; emit scoped output. Characterization
   tests already pin both halves.
2. **media player** (`elacity-player`): the `render_only` segment-streaming
   consumer contract; never receives CEK.
3. **non-media player** (`ddrm-viewer`): `wasm-renderer` render tiers
   (pixel-lock / html-lock + watermark) as a `viewer` capsule.
4. **encrypt/creator path** (`cenc-encrypt`): CEK/KID-in-wasm packaging
   (Invariant 1), when the creator flow opens.

## Viewer seam — implemented (B1–B3, 2026-06-11)

The downstream (player) seam is now built end-to-end and gate-green; only the
upstream live CEK producer remains gated (the same Anders dependency as the rail).

- **B1 — `elacity-player` capsule** (`capsules/elacity-player/`): a `viewer` capsule
  that plays fMP4/CENC via **Media Source Extensions**, fetching an init segment +
  ordered media segments over an opaque session handle. It satisfies
  `elastos.viewer/media@1` and proactively **fails closed** if any forbidden key
  field (`cek/iv/wrapped_cek/...`) appears in what it receives.
- **B2a — decrypt-provider `stream_segment` op** (feature `rail-stream`): given an
  opened session's sealed material + a segment index, the boundary unwraps the CEK
  **once in-VM** (`Zeroizing`), decrypts **only that segment**, and returns its bytes
  as the scoped `stream` output. CEK/IV never cross the boundary; only one segment's
  plaintext exists per call; a reordered/substituted/expired set fails closed before
  any byte is returned. Ladder rung pinned at **79** + wasm build green.
- **B2b — runtime scoped media routes** (`elastos-server`
  `api/viewer_media.rs`): `GET /api/viewers/:viewer/media/:session` (manifest,
  metadata-only), `/init` (clear init bytes), `/segment/:index` (decrypted bytes
  relayed per-request via `stream_segment`). The runtime holds **only** the CEK-free
  sealed material + clear init bytes + public metadata — never the CEK, never
  decrypted media. Range/expiry are enforced before any relay; the provider response
  is scanned for forbidden key fields (defense in depth); auth is the
  `x-elastos-home-token` launch token scoped to the viewer, and the token principal
  must own the session.
- **B3 — routing decision**: `viewer_for_required_interface("elastos.viewer/media@1")
  → "elacity-player"` + the player launch route builder. The **live session
  creation** (populating a `MediaSession` from the `drm→rights→key→decrypt` open
  chain) is **backend-gated**: the open chain bails at `key.release` =
  `not_configured` until a real CEK producer (local KMS / dKMS / Lit adapter) is
  wired behind `key-provider`. No additional player-side engineering is required —
  it is the A2/A3 flag-flip.

**What "until the backend lands" means:** the player ⇄ runtime ⇄ decrypt-provider
seam is complete and tested. Playback goes live when (a) the shipped decrypt-provider
is built with `rail-stream` and (b) `key-provider` seals a real CEK to the published
decrypt-session key. For a single owner on one machine this is a **local** producer +
config flip — **not** a server deployment.

## Non-goals here

- No on-chain access logic in the decrypt-provider (that is rights/key upstream).
- No change to the trusted core — dDRM is a capsule-tier concern by principle.
- No live rail wiring until the two Anders decisions land.
