# `/api/market/*` — canonical contract (SSOT for the shell ↔ gateway seam)

> The single vocabulary the storefront shell (`capsules/marketplace-content/browser/api.js`) and the
> gateway (`gateway.rs` → `gateway_marketplace.rs` / `viewer_open.rs`) MUST share (P10 one canonical path,
> P12 docs==code). The 15-seat council flagged that the two were built to **different** route sets hidden
> behind the shell's mock fallback ("browser-verified ≠ wired"). This doc reconciles them and marks each
> route **BUILT** (served today) or **PENDING** (shell SPECs it; not yet served). The money path NEVER
> trusts discovery — buy re-verifies terms live (Phase-1).

## BUILT — served by `gateway.rs` today
| Method · Route | Handler | Request | Response | Auth |
|---|---|---|---|---|
| `GET /api/market/search?op&q` | `market_search` | query | `{listings[], indexed, coverage}` | public* |
| `GET /api/market/sections` | `market_sections` | — | `{sections:[{id,title,...}]}` | public* |
| `POST /api/market/order/sell` | `market_order_sell` | `{gateway?, ledger, token_id, quantity, price, pay_token?}` | `{unsigned_tx{to,data,value,selector,note}}` | **Home token** |
| `POST /api/market/order/withdraw` | `market_order_withdraw` | `{gateway?, operative, token_id, quantity}` | `{unsigned_tx}` | **Home token** |
| `POST /api/market/order/approve` | `market_order_approve` | `{operative, gateway?}` | `{unsigned_tx}` | **Home token** |
| `POST /api/market/buy` | `buy_owned_access` | storefront: `{content_id, operative, token_id, ledger, quantity?, seller?, expected_price?, expected_pay_token?}` · legacy re-buy: `{uri}` | buy outcome / unsigned tx | **Home token** |
| `GET /api/market/get?operative&token_id` | `market_get` | query | `{on_chain{token_id,seller,price,pay_token,supply_left,has_access}, sellers, coverage}` — live `sellersOf`+`listings`, lowest active price | public* |
| `GET /api/market/vault` | `market_vault` | — (principal from token) | `{owned:[{uri,name,content_cid,mime,acquired}], count, source}` — the buyer's Library Acquired assets | **Home token** |
| `POST /api/market/acquire` | `market_acquire` | `{content_id (KID), content_cid (CID), uri?, metadata?}` | `{object, uri, content_cid, availability}` — gates `hasAccessByContentId` then dispatches the Acquire op | **Home token + on-chain entitlement** |

\* discovery is public but rate-bounded by a 10s in-process TTL cache (`recent_index_cached`) so an
unauthenticated burst collapses to one chain sweep. `get` takes `operative`+`token_id` (the shell has both
from the index listing) — `has_access` (the bytes16 KID lookup) is the enrichment follow-on.

**`POST /buy` (storefront, not-yet-owned):** the shell sends the asset's on-chain identity
(`operative`+`token_id`+`ledger`, all from the discovery listing). On the live `chain` path the gateway
sources `seller`/`price`/`payToken` LIVE from `sellersOf`/`listings` (keyed at ACCESS_TOKEN id=1) — picking
the lowest active seller — so **no `ELASTOS_DDRM_BUY_*` env pins are required** (env still overrides for
dev/fixtures). `expected_price`/`expected_pay_token` (what the buyer saw, from `/get`) arm **abort-on-drift**:
the live re-read at buy time must match or the buy fails closed before signing. `buyAccess` binds the real
content `tokenId`; the buy stays UNSIGNED (external wallet only on a release build). Omitting `content_id`
falls back to the legacy `{uri}` re-buy for an object already in the Library.

## PENDING — the shell SPECs these; gateway does NOT serve them yet (mock-only in the shell)
| Method · Route | Purpose | Lands in |
|---|---|---|
| `GET /api/market/get?content_id` | by-`content_id` variant (needs KID/metadata enrichment so the index carries the CID); the by-`{operative,token_id}` variant above is BUILT | **Phase 2** enrichment |
| `GET /api/market/listed` · `/history` | the buyer's listed / activity views | **Phase 2** |

## Name-fork reconciliation (the shell's old verbs → the canonical built routes)
- shell `order/assemble` (a **buy** assembly) → canonical **`POST /buy`** (the built buy path). "assemble" was a
  buy verb; the built surface splits buy (`/buy`) from resale (`/order/*`). Use `/buy`.
- shell `order/cancel` → canonical **`POST /order/withdraw`** (same `withdrawListing` op; renamed).
- shell had **no resale-list call** → canonical **`POST /order/sell`** (+ prerequisite `POST /order/approve`).
  The "List for resale" flow must call these two.

## Listing schema gap (reconcile in Phase 2)
`content_index::Listing.to_json` (schema `elastos.market.listing/v1`) emits
`{channel_address, operative_address, token_id, token_uri, op_type, content_id, metadata_status}` — it has
**no** `name`/`medium`/`tier`/`listings[]`/`copies` that the shell's `cardHTML`/`renderAsset` read. Today
`metadata_status:"needs_kid"` (name/medium are empty on the live path). **Phase 2 KID/metadata enrichment**
fills `name`/`medium`/etc. from the asset's `tokenURI` metadata; until then the shell must tolerate the lean
schema (render placeholders) rather than assume the mock shape. Add a serde round-trip/schema test so the two
shapes cannot silently drift again.

## Rule
A shell call may set `live=true` ONLY for a **BUILT** route. PENDING routes stay mock and must be visibly
labelled mock — never let a 404 fall back to mock that masquerades as wired.
