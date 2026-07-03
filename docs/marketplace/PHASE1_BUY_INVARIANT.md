# Phase 1 — Bind buy terms to the re-verified on-chain listing (the money-path invariant)

> **The single most important change in the entire marketplace effort, and it gates everything else.** Hand-to-Cursor spec. Implements on `feat/marketplace-runtime`; reviewed; merged into the dDRM line. Until this passes, **no buy button ships — not even in a demo.** Contract: [PRINCIPLES.md](../../PRINCIPLES.md) §11 (fail closed). Plan context: [PLAN.md](PLAN.md) §0, §4.

## Implementation status (this branch — compile + unit-tested; live = Cursor)
**Core + abort-on-drift IMPLEMENTED** in `buy_authority.rs` + `chain_tx.rs` (cargo build --tests 0 warnings; 7 `buy_authority` tests pass):
- ✅ Real tokenId bound via the chain-provider `resolve_token_id` op (KID → `AssetCreated` + mint calldata) or a pinned override; **fail-closed** if unresolved — never `word_from_id(content_id)`.
- ✅ `value = price × quantity`; ERC-20 approve leg = `approve(Operative.paymentProcessor(), total)` (not the gateway).
- ✅ **Abort-on-drift**: bind terms → re-read `listings(operative,tokenId,seller)` live before signing → `ensure_no_drift` (fail-closed on price/pay-token drift) → abort if sold out (supply 0).
- ⏳ **Cursor's live-chain pass**: the real `eth_getLogs`/`eth_call`/broadcast against Base + the 3 `#[ignore]`d integration tests.

## The bug (confirmed in code)
`elastos/crates/elastos-server/src/api/buy_authority.rs` assembles the purchase tx from **`ELASTOS_DDRM_BUY_*` env-pinned terms**, and **`tokenId` defaults to `word_from_id(content_id)`** (`:230-246`) — a hash of the content id, **not the real on-chain tokenId**. Consequence (the worst failure a marketplace can have): a byte-correct `buyAccess` tx is signed and broadcast against a fabricated tokenId / stale price, the wallet **debits**, the tx **confirms** (on-chain finality, no refund), and `has_access_by_content_id` for the asset stays **false** → the open **fails closed**. **Paid + no access + no refund.**

## The fix — one new order-assembly step + one hard invariant
Insert a **re-verify-then-bind** step between "user clicks buy" and "assemble the unsigned tx":

1. **Re-decode the listing from chain** (not the index cache): call `content-market.reconstruct_listing` / `listing_from_event` on the asset's mint calldata/event, via `chain-provider`.
2. **Re-read live on-chain state** via `chain-provider`: the real `tokenId`, current `price`, `payToken`, remaining `supply/copies`, and the listing `seller` — bound to the asset's **on-chain** seller (not a caller-supplied or env value).
3. **Bind those exact values into the `UnsignedPurchaseV1`** that gets signed — replacing every `ELASTOS_DDRM_BUY_*` env read and the `word_from_id(content_id)` tokenId default.
4. **Abort on drift (the invariant):** immediately before signing/broadcast, re-read the same fields once more; if `(seller, tokenId, price, payToken, supply)` differ from what was bound at assembly, **fail closed — do not broadcast.** (Optional hardening: also rely on the contract reverting if terms changed, but do not depend on it alone.)

Keep everything else as-is: `wallet-provider` signs (human-in-loop), `chain-provider` broadcasts, await receipt, `has_access` flips true. The CEK path is untouched (P15).

## Grounded buy-call shape (verified against elacity-web v3 — `BuyMediaView` → `MediaContext.handleAccessTokenPaymentAsync`)
The real v3 buy is `AuthorityGateway.buyAccess(...)` with two overloads chosen by pay-token. The runtime's **selectors, gateway `0x09dBe7`, USDC `0x833589fC`, chain 8453, and arg order are already correct**; the **bound values** are what's wrong today. Each bound field, grounded:
- **`tokenId` = the ledger MEDIA tokenId** (`id.toBigNumber()` from the asset/route) — **NOT `word_from_id(content_id)`** (a content hash, unrelated to the on-chain id). This is the core fix (binding a fabricated tokenId = paid+no-access). Resolve it from the listing/asset or via the KID→ledger-tokenId resolver (scan `DigitalAssetRegistered`, which carries both `tokenId` and the `bytes16 contentId`).
- **`ledger` = the ERC-1155 channel ledger** (`metadata.properties.ledger`, e.g. `0x6756e140…`) — **NOT the gateway.** Runtime currently defaults `ledger = to` (the gateway) when unpinned → wrong recipient.
- **`seller` = the on-chain listing seller** (from `sellersOf`/`listings` or the listings endpoint), not a caller/env value.
- **`pricePerToken` = the listing price** in pay-token minor units (USDC = 6 decimals); **terms source** = `GET /2.0/authority/{authority}/listings/{operative}/1` (the trailing `/1` = the ERC-1155 ACCESS_TOKEN role id) **or** re-read live from chain. The runtime has no listing-resolution seam today.
- **ERC-20 overload** `buyAccess(seller,ledger,tokenId,quantity,pricePerToken,payToken)` `0x0ede2294`, `value = 0`, **requires a prior `approve(spender, MaxInt256)` where `spender = the OPERATIVE's `paymentProcessor()`** (read live from the operative — **NOT the gateway**), gated by `allowance(account, paymentProcessor)`. Runtime today only emits a `requires_erc20_approve` boolean and never models the `paymentProcessor` spender → **live USDC buys would fail.**
- **Native overload** `buyAccess(seller,ledger,tokenId,quantity,pricePerToken)` `0xf7580ad9`, **`value = pricePerToken × quantity`** (runtime parses `ELASTOS_DDRM_BUY_PRICE` only — **missing the ×quantity multiplier**).
- **`authority` (the gateway) is per-asset** = `metadata.properties.authority` (the runtime's pinned `0x09dBe7` matches current assets, but the assembler should source it per-asset, not hard-pin).

## Files
- Edit: `elastos/crates/elastos-server/src/api/buy_authority.rs` (the env-pinned terms + `word_from_id` tokenId at `:230-246`; the assemble→sign→broadcast flow).
- Call: `capsules/content-market` (`reconstruct_listing` / `listing_from_event`) for the re-decode; `chain-provider` for the live reads (`prepare_transaction`/typed reads, `has_access_by_content_id`, receipt).
- Do **not** add chain RPC to `buy_authority` itself — go through `chain-provider` (sole RPC declarant; P3/P4).

## Pass/fail checks (each independently verifiable)
1. **Wrong-token cannot broadcast:** a buy whose bound `tokenId` ≠ the asset's real on-chain tokenId is **refused before broadcast** (unit/integration test on `chain-mock`). *This is the core regression test.*
2. **Drift aborts:** if price/supply/seller changes between assembly and the pre-broadcast re-read, the order **fails closed**, no tx is sent.
3. **Happy path end-to-end:** not-owned → buy (re-verify → sign → broadcast → receipt) → `has_access_by_content_id` true → `drm.open → key.release → decrypt.render` succeeds for a Tier-2 asset, on `chain-mock` **and** against a pinned testnet contract.
4. **No env terms on the live path:** grep confirms the live buy path no longer reads `ELASTOS_DDRM_BUY_*` for `seller/tokenId/price/payToken` (env may remain only for dev/chain-mock fixtures, fenced).
5. **CEK untouched:** no new code path touches the CEK; `key-provider` still releases session-wrapped only.
6. **ERC-20 approve targets `paymentProcessor`:** the assembled approve (when `payToken != native`) has spender = the asset's Operative `paymentProcessor()` (read live), **not** the gateway; allowance pre-checked. A buy assembled without it on an ERC-20 asset is refused.
7. **Native value = price × quantity:** the native-overload `value` equals `pricePerToken * quantity` (not a flat env price); a quantity > 1 native buy binds the correct total.
8. **Real tokenId + ledger bound:** the bound `tokenId` is the resolved ledger media tokenId (not `word_from_id`) and `ledger` is the channel ERC-1155 (not the gateway); both sourced from the asset/listing, asserted in the wrong-token test.

## Gate
`just test-crate elastos-server` green (incl. the new wrong-token + drift tests); `just alignment-check` OK; changed files clippy-clean. New tests start as real assertions (this is a correctness fix, not a ratchet).

## Sequencing note
Run **in parallel with P1b** (harden the open boundary — PQ-hybrid threshold release + dKMS v0), the largest item and an external-audit dependency. P1 makes a purchase *provably grant what it charges for*; P1b makes the grant *openable on a real chain*. Both are prerequisites to any UI; both have no UI dependency.
