# marketplace-content (DRAFT)

The marketplace shell — **discover, buy, trade, list, withdraw** dDRM assets (content now; apps & games later, same storefront). A **pure UI capsule**: it reads `elastos://market/*`, renders, requests **unsigned** orders, and routes signing to the wallet. On buy it **triggers a pin of the encrypted asset into your local Library**; opening is handed off to the existing **player**, minting to the **creator app**. It **mints nothing, plays/renders nothing**, and holds **no signer, token, CEK, or chain RPC** (Principle 16). Canonical scope: `docs/dkms/COMMERCE.md`.

## Run it standalone (review)
Open `browser/index.html` in a browser. It runs against an embedded mock (`mock.js`) that also **documents the `elastos://market/*` contract** (see `api.js`). The wallet pill shows **demo** when no gateway answers, **live** when one does.

- **Discover** — asset-type (Content; Apps/Games soon) + medium sections (Watch/Listen/Read/View/Explore), facets, search, access-right chips, poster/cover art.
- **Asset detail** — poster/cover ("Identity verified · contentId == KID") + commerce/trust sidebar (buy box, royalty splits, About, Provenance). Owned → "Open in your library" (hands off to the player); the shell renders nothing.
- **Buy** — assembles an **unsigned** order **routed to the wallet** (the shell never signs); on success the encrypted asset **pins to your library**; surfaces the Phase-1 invariant (terms re-verified from chain, abort on drift).
- **Vault** — Owned / Listed (manage resale listings) / History. Minting lives in the creator app, not here.

## The contract this shell expects (for the index + gateway)
`api.js` is the source of truth: `GET market/sections` · `GET market/search` · `GET market/get?content_id` (returns the listing **plus a live re-verified `on_chain` block** — never trusted from cache) · `POST market/order/assemble {content_id,quantity,seller}` (returns an **unsigned** tx) · `POST market/order/cancel` (unsigned `withdrawListing`) · `POST market/acquire {content_id}` (triggers the buy→pin to the local Library) · `GET market/vault`.

## Status / follow-ups
- Frontend is complete, runnable, and **wired to the live gateway** (mock fallback for standalone review).
- The thin `marketplace-content.wasm` launcher is **built** (`wasm/main.rs` + `Cargo.toml`, mirrors the `marketplace` capsule; compiles `--release`).
- Remaining: KID/metadata enrichment (name/cover/`content_cid` from `token_uri`), the live-chain test (Cursor), and launcher registration.
- Backend: `content-index` (polling cache over `content-market`) + the `/api/market/*` gateway routes (Phase 2); the buy path's re-verified-listing binding (Phase 1) is the gate before the buy button does anything real.
- See `docs/dkms/COMMERCE.md` (canonical scope, buy invariant, acquire + trade), `docs/dkms/COMMERCE_API.md` (the `/api/market/*` contract) and `docs/dkms/COMMERCE_CONTRACTS.md` (selectors, addresses, ABIs).
