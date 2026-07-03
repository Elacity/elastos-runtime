# Phase 2 — content-index (polling cache) + the `/api/market/*` gateway routes

> Hand-to-Cursor spec. Builds the **discovery layer** the shell (`capsules/marketplace-content`) already consumes via `browser/api.js`. Implements on `feat/marketplace-runtime`. Contract: [PRINCIPLES.md](../../PRINCIPLES.md); plan: [PLAN.md](PLAN.md) §2–§3. **Not a money-path change** — discovery only; the money path re-verifies live ([PHASE1_BUY_INVARIANT.md](PHASE1_BUY_INVARIANT.md)).

## Shape (red-team-corrected)
The index is **NOT a new provider capsule** — it's a **server-side cache table inside the gateway** (`elastos-server`), fed by a polling job that calls `content-market` (decode) over `chain-provider` (the sole RPC). It holds **no keys, no RPC of its own, no write authority** — it's a query accelerator *below* the canonical calldata path (P10), never a trusted oracle. `content-market` already owns `elastos://market/*`; the gateway serves `/api/market/*` from the cache. No namespace collision, no new capsule surface.

## Chunk 1 — the listing row + cache table
Define `MarketListingV1` = exactly what `content-market` emits (`content_id`==bytes16 KID, `channel_address`, `chain_id`, `token_uri`, `metadata_cid`, `op_type`+code, `copies`, `price_wei`, `pay_token`, `name`, `description`, `image_url`, `mime_type`, `asset_type`, `creator_address`, `metadata_status`, `source`) **plus index-derived** (`tier` from asset_type/mime, `medium` ∈ {watch,listen,read,view,explore}, `first_seen_block`, `listings[]` primary+resale, `resale_floor`, `holders`). Persist one row per `content_id`; every row carries `source` so it's re-derivable. *Check:* a fixture mint round-trips calldata → `content-market` decode → row → JSON matching `api.js`'s shape; serde + a schema test pass.

## Chunk 2 — the polling indexer (NOT subscription)
`chain-provider` exposes `eth_getLogs` only (no `eth_subscribe`), capped to **10k-block windows** on a curated RPC subset. Build a poll loop: maintain a cursor; each tick request `getLogs(DigitalAssetRegistered, AssetCreated)` over `[cursor, min(head, cursor+10k)]`; for each event call `content-market.listing_from_event` then enrich via `ipfs-provider` + `content-market.enrich_listing` (which enforces `metadata.kid == calldata contentId` — keep that reject). Upsert rows; advance cursor. Handle: **backfill** (initial sweep from a configured genesis block), **dedup** (idempotent upsert by `content_id`+`tokenId`), **reorg rollback** (on a head reorg below a confirmed depth, re-derive affected rows by `first_seen_block`), and a **freshness SLO** ("indexed within N seconds," configurable — *not* "one block"). RPC-cost/rate-limit aware (the curated subset; backoff). *Check:* a minted asset appears in the cache within the SLO; a simulated reorg re-derives the affected rows; the loop survives an RPC window that returns nothing / rate-limits.

## Chunk 3 — the `/api/market/*` gateway routes (serve the shell contract)
In `gateway_marketplace.rs` (extend; it already serves the app catalog), add read routes over the cache:
- `GET /api/market/sections` → `{ sections:[{id,title,listings:[MarketListingV1]}] }`
- `GET /api/market/search?medium&q&op&sort&cursor` → `{ listings, cursor }` (faceted; stable cursor pagination from the cache)
- `GET /api/market/get?content_id` → `{ listing, on_chain }` where **`on_chain` is read LIVE from `chain-provider` at request time** (`token_id`, `price`, `pay_token`, `supply_left`, `seller`, `has_access_by_content_id`) — **never from the cache**. This is the trust hinge: detail + buy use live terms, the cache is only for browse.
- `GET /api/market/vault?wallet` → `{ owned:[MarketListingV1] }` (rows where `has_access` true for the wallet)
- `POST /api/market/order/assemble {content_id,quantity,seller}` → `{ unsigned_tx }` — delegates to the Phase-1 re-verified order-assembly (returns UNSIGNED; never signs). Gate behind the same auth as other viewer routes.
*Check:* the shell (`marketplace-content`) flips its pill from **demo** to **live** and renders real listings; `get` returns live on-chain terms that can differ from the cached row (prove the cache is non-authoritative); `order/assemble` returns an unsigned tx and never touches a key.

## Chunk 4 — wire the shell to live data
Point `marketplace-content/browser/api.js` at the real routes (it already tries them first, falls back to mock). Remove the mock from the shipped build (keep it under a `?mock=1` dev flag). *Check:* with the gateway up, Discover/Search/Asset-detail render from the chain-derived cache; with it down, the dev mock still runs for offline review.

## Honest limits (carry in code + docs)
- **Freshness is bounded by polling**, not instant — state the SLO; never imply real-time.
- **The cache is centralized-but-verifiable** — same trust shape as a subgraph; the guarantee is re-derivability + live re-verify at point-of-use, not "no chokepoint."
- **IPFS enrichment may lag/fail** — rows persist with `metadata_status: unresolved`; the UI shows "metadata unavailable," identity + sell terms survive from calldata.

## Sequencing
Chunk 1 + 3 are tractable immediately (schema + routes over `content-market`). Chunk 2 (the poll loop) is the engineering core. Chunk 4 is a one-line flip once the routes are live. Independent of Phase 1 (discovery vs money path) — can build in parallel, but the buy button stays dark until Phase 1 lands.
