# Phase 2 — listing enrichment (name / cover / content_cid) — turnkey spec

> The lean discovery `Listing` (operative/token_id/op_type/token_uri) → a RICH listing the shell renders +
> can acquire. **The enrichment logic already exists** — `content-market`'s `enrich_listing` op. This wires
> it into `/get`. It is a **LIVE multi-provider orchestration** (two network fetches + a subprocess fuse),
> so it is Cursor-built + verified against real assets — not a blind compile-build (verify, don't trust).

## What `content-market` gives us (already built + unit-tested)
`Request::EnrichListing { request: EnrichRequestV1 }` (capsules/content-market/src/main.rs:54, 225). It is
**PURE — it fetches nothing**; the caller hands it everything:
```rust
EnrichRequestV1 { calldata, channel_address, chain_id=8453, expected_selector?, metadata }
```
It re-derives the contentId from `calldata` (authoritative), **requires `metadata.kid == contentId`** (else
`identity_mismatch`), then attaches descriptive fields. Output `ContentListingV1` (verified by its tests):
```
{ content_id (==KID), name, description, image_url, content_cid, mime_type, asset_type, creator_address, op_type, … }
```
Metadata field paths it reads (PC2 `ContentIndexerService.ts:1102`): `name`, `description`,
`image` (else `media.previewURL`) → `image_url`; `media.uri` → `extract_cid` → **`content_cid`** (the
encrypted asset CID the buy→pin needs); `media.contentType` → `mime_type`; `kid` (or `properties.kid`);
`properties.publisher` → `creator_address`.

## The orchestration to build into `/get` (or a new `/api/market/enrich`)
The shell already has the lean fields; `/get` adds the live terms (built) + should add the rich fields:
1. **Inputs from the index `Listing`** (AssetCreated): `channel_address`, `token_uri`, and the **mint tx hash**
   (the AssetCreated tx — the resolver already fetches its input via `chain-provider tx_input`).
2. **Fetch the mint calldata** — `chain-provider` `tx_input(mint_tx_hash)` (already used by `resolve_token_id`).
3. **Fetch `metadata.json`** — `extract_cid(token_uri)` → fetch that CID via the `content/*` plane
   (`fetch_bytes_via_provider` / ipfs-provider) → `serde_json::from_slice`. (P4 — content plane, not raw ipfs.)
4. **Fuse** — call `content-market` `{op:"enrich_listing", request:{calldata, channel_address, metadata}}` →
   the rich `ContentListingV1`. Fail closed on `identity_mismatch` (a metadata that lies about its KID).
5. **Merge** into the `/get` response (`name`, `image_url`, `content_cid`, `mime_type`, `content_id`) alongside
   the already-built live `on_chain` terms. The shell's `normalize()` already tolerates these fields.

## Why it is Cursor's (the live boundary)
- Two **live fetches** (mint tx-input via chain-provider; `metadata.json` via ipfs-provider) — must run against
  a live node + **real assets** to confirm the `media.uri`/`kid` paths hold for production metadata.
- `content-market` is a **subprocess capsule** — confirm the gateway can invoke it (a `run_capsule`-style call
  or a registered provider) the same way it invokes `chain-provider`; wire that transport if absent.
- Performance: enrich **lazily on `/get`** (one detail view = one enrichment), not for every discovery card.

## What this unblocks
Real names/covers on cards + detail; and **`content_cid` for the live buy→pin** (today the shell's `acquire()`
falls back to mock because the lean listing has no `content_cid` — this fills it). After this, the live loop
discover → detail → buy → **acquire (real pin)** → vault → open is complete end-to-end.

**Also closes the deep-audit KID/CID binding finding (LOW):** once the server resolves the canonical
`content_cid` for a `content_id` (KID) from the asset metadata here, `market_acquire` can **ignore the
client-supplied `content_cid`** and pin only the canonical CID — removing the "entitled buyer pins an
arbitrary CID" gap. Until then it is bounded (opaque ciphertext + open re-gates on the embedded KID) and the
fetch is size-capped (`library_acquire`, `ELASTOS_DDRM_ACQUIRE_MAX_BYTES`).
