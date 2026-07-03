# Marketplace in the Runtime — The Plan

> ⚠️ **SCOPE SUPERSEDED — see [SCOPE.md](SCOPE.md).** This plan predates the scope correction: the marketplace is **buy/trade ONLY** — it mints nothing (creator app) and plays nothing (runtime players); on buy the encrypted file pins to your Library and opens in the existing player. Where this doc folds the creator in as "Studio" or shows in-app secure-render, SCOPE.md governs. Kept for the architecture/contract/UX reasoning, which remains valid.

> **Status: design + review artifact on `feat/marketplace-runtime` (branched off the dDRM line; merge back when green).** Authored by the orchestrated council (audit → 4 design lenses → synthesis → red-team), grounded in the actual runtime code + a final-research pass over `Elacity/elacity-web@release/go-live-integration`. Nothing here is implemented yet; the verbs are REUSE / WIRE / BUILD as marked. Correctness contract: [PRINCIPLES.md](../../PRINCIPLES.md). Companions: [PHASE1_BUY_INVARIANT.md](PHASE1_BUY_INVARIANT.md), [layouts.html](layouts.html).

## 0. Verdict (read first)
The runtime is **not missing a marketplace** — it's missing **two layers on top of one that half-exists, and the half that exists is the right half.**
- **REAL → REUSE + WIRE:** producer spine (creator → publish → chain), the discovery decoder `content-market` (pure calldata→`ContentListingV1`, KID-authoritative, fail-closed, **29 tests**), the open path `rights → drm → key → decrypt`, and tiered secure render (`decrypt-provider`, Tiers 1/2/2b/5).
- **LESS REAL → BUILD-the-safety-first:** the trade/open *money path*. **The landmine (confirmed `buy_authority.rs:230-246`):** buy terms are `ELASTOS_DDRM_BUY_*` env-pinned and `tokenId` is derived from `content_id`, **not** the real on-chain tokenId → a "successful" buy can debit the wallet, confirm on-chain, and the open **fails closed**: *paid + no access + no refund.* Fixing this is **#1, before any buy button — even a demo** (see [PHASE1_BUY_INVARIANT.md](PHASE1_BUY_INVARIANT.md)).
- **The two genuine holes:** no queryable **listing index**; no public **marketplace UI**.

**Two non-negotiables** (PRINCIPLES + the real `capsule.json`s): the **chain is the only source of truth, the index is a re-derivable cache**; and the **CEK never enters the buy/trade path** (`key-provider` releases session-wrapped, never raw).

## 1. Architecture — reuse vs rewrite
**Keep the Rust/WASM provider spine untouched. Port only UI *behavior* from elacity-web. Discard the centralized backend.** You are *not* rewriting the marketplace in Rust — the Rust (trust/data providers) exists; the UI is a browser-frame capsule, like the current `marketplace` capsule.

Final-research correction (from `elacity-web@go-live-integration`): the backend is **REST (`axios`, per-wallet JWT) + direct `ethers` on-chain reads + Particle account-abstraction — NOT GraphQL** (the earlier "GraphQL backend" was stale memory). So what we discard is a **REST metadata/identity service**, which the runtime already replaces: content-market + ipfs for metadata, chain-provider for on-chain reads, capability tokens for the JWT.

| From | Verdict |
|---|---|
| `content-market` decoder + the 6 providers | **KEEP — the trust root** |
| elacity-web `MediaTradeView`/`BuyMediaView`/`OrderBooks` *behavior* | **PORT behavior** (pure UI, no authority) |
| The REST metadata/JWT backend + any play-time license server | **DISCARD** (centralized; license-at-play violates the on-device provider model) |
| Role-token economics + opType `{FREE,BUY_ONCE,BUY_AND_RESELL}` | **KEEP the semantics** (already mirrored) |

## 2. Capsule structure
**One marketplace SHELL capsule (`app`) with media SECTIONS. Assets are sealed `.ddrm` *objects*, not capsules (Principle 13). The shell dispatches to the existing per-tier viewers.** A capsule-per-media-type would duplicate the `is_pixel_lock` containment N times (fights P5/P10/P13).

**Red-team correction:** the listing index is **NOT a new provider capsule** (that over-builds and collides — `content-market` already owns `elastos://market/*`). It is a **server-side cache table behind the existing gateway** (`gateway_marketplace.rs`), fed by a polling job that calls `content-market`. It holds no keys because it's a query cache; it needs no capsule isolation.

Roles: `marketplace-content` (NEW shell, holds no signer/token/CEK) · `content-market` (KEEP, decoder/trust-root) · `creator` (KEEP → folds in as "Studio") · `chain/wallet/rights/drm/key/decrypt/publish-provider` (KEEP) · `ddrm-viewer` + `elacity-player` (KEEP, tier dispatch) · `object-provider` + `library` (KEEP → "Vault").

## 3. Data model — where it reads from
**Chain calldata/event decode (`content-market`) = verifiable trust ROOT. The index = re-derivable CACHE. The money path re-verifies against chain and never trusts the cache.**

Two red-team corrections, baked in:
- **Polling, not subscription.** `chain-provider` has **no event-subscription primitive** — only `eth_getLogs` capped to 10k-block windows on a curated RPC subset (public endpoints rate-limit getLogs). Budget for poll cursors, backfill, **reorg rollback** (`first_seen_block`), and a freshness SLO ("within N seconds," not "one block").
- **Honest framing:** the index is **centralized-but-verifiable**, not "no chokepoint" — same trust *shape* as a subgraph, but every row re-derivable from calldata and the **money path re-decodes at point-of-use** (so a stale/malicious index can't cause a wrong buy). *That re-verification does not exist today — it is build item #1.*

## 4. Trade model — use-without-holding
**A buy writes an on-chain access-token grant; consumption reads it. The CEK is never traded.** The tradeable thing is the on-chain *role* (decrypt right / royalty share / resale right). Compose with **zero new key authority**: `buyAccess` (write grant) → `has_access_by_content_id` true → `drm.open → key.release (session-wrapped) → decrypt.render`.

Final-research insight: production elacity-web has **fixed-price listings + auctions + an order book / offers** (`Governance/OrderBooks/`, an `Auction` contract). The runtime's current fixed-price `buyAccess` is the **MVP subset** — design the trade primitive so **auctions/offers layer on later** (don't lock into fixed-price-only). Contract topology = the Artion suite (`NFT + Marketplace + Auction + Factory + PrivateFactory + ArtFactory`); pin the **real Base addresses from the `base-network` deploy config** in Phase 7.

**THE invariant (do first):** the signed `buyAccess` tx carries the exact `(seller, tokenId, price, payToken, quantity)` **re-read from chain at assembly time**, and **aborts if a post-assembly re-read differs**. Resale/royalties are on-chain-enforced each hop (require `has_access` proof before assembling a resale order). **Honest tensions, never overclaim:** dKMS is operator-curated 2-of-3 → "keys used, never owned"; never "fully decentralized/uncopyable." And the pure on-chain model **has no revocation/takedown story** (the discarded license server had central revocation) — a real product/legal gap to own.

## 5. UX / IA + layouts
One unified **Discover** storefront; sections by **EXPERIENCE/MEDIUM**, not file extension or opType; the **Access Receipt** is the first-class connective object. Nav: `Discover · Vault · Studio · Activity`; an **access-right chip** on every asset ("you buy access, not the file"). Sections: **WATCH** (video) · **LISTEN** (audio) · **READ** (epub/pdf/text/comic) · **VIEW** (image/svg) · **EXPLORE** (3D) → tier → viewer dispatch; opType/chain/creator/price as facets+shelves. **Launch only Tiers 1/2/2b/5**; gate creator-upload mimes to what secure-view covers. The centerpiece is **Asset Detail = "secure stage + commerce/trust sidebar"**, and the one genuinely new primitive is the **secure preview** (show-but-not-extract, watermarked, via `decrypt-provider`'s existing egress under a narrow FREE-preview rights scope — *no new bytes path*). See [layouts.html](layouts.html) for the concrete mock.

## 6. Honest scrutiny — PC2 vs elacity-web vs runtime-native
PC2 got the **economics + tier semantics** right, the **plumbing** wrong (centralized index + play-time license server). elacity-web got the **trade UX** right (centralized REST/JWT data + ethers reads). The runtime-native spine got the **trust model** right. Keep PC2's semantics, port elacity-web's UX behavior, build discovery + UI the runtime-native way. **Two over-discards to NOT lose:** PC2's *operational* indexing engineering (cursor stability, backfill, reorg handling) and the license server's **revocation** hook.

> **Evidence caveats:** elacity-web findings are grounded in the actual `go-live-integration` branch. Some PC2 *internals* rest on design-agent exploration + model memory — treat as directional, verify before relying. The red-team verified all runtime claims against the code and corrected: it's **29 tests** (not 27); "the marketplace is a WebSpace" (ROADMAP) means an **app-store of installable capsules**, not the content marketplace; the buy path does **not** re-verify against chain today; there is **no** event-subscription primitive.

## 7. Phased path (money-path-first)
- **P0 — verify, don't rebuild.** `content-market` (29 tests), `buy_authority` chain-mock loop, `decrypt-provider` tiers. `just verify`. No code.
- **P1 — bind buy terms to the re-verified on-chain listing + abort-on-drift.** DO FIRST; gates everything. No buy ships before this. → [PHASE1_BUY_INVARIANT.md](PHASE1_BUY_INVARIANT.md)
- **P1b (parallel — the real project, under-scoped):** harden the open boundary (PQ-hybrid threshold release + dKMS v0). Months + external-audit dependency; **not** a peer to building UI.
- **P2 — the polling index** as a cache-table behind the gateway (getLogs windows, cursors, reorg rollback, freshness SLO).
- **P3 — the secure-preview FREE scope** (a real new trust boundary — audited like the paid open path).
- **P4 — the `marketplace-content` shell** (port elacity-web behavior; dispatch to viewers by tier; holds no authority).
- **P5 — buy end-to-end** (optimistic pre-flight: balance/allowance/supply via chain-provider before submit).
- **P6 — resale + Vault + Studio + royalty surfacing.**
- **P7 — identity/anti-sybil, AV per-buyer fingerprint (when the MSE ceiling is addressed), IPFS resilience, pin Base contract addresses, external audit, honest launch copy.**

**Do first, in parallel:** P1 (bind buy terms) + P1b (open boundary) — the two genuine prerequisites with no UI dependency.

## Council (who designed this)
Principal systems architect · capability-security/runtime architect · applied cryptographer (dDRM) · smart-contract & trade-systems engineer · frontend↔backend integration lead · principal product designer (marketplace) · information architect & interaction designer · content-protection/forensics specialist · web3 access-economy strategist · red-teamer · orchestrator/process steward.

## Workflow
Claude specs/audits + produces layouts (read-only on the runtime); Cursor implements + signs off; the human holds the merge. All work lands on `feat/marketplace-runtime` and merges into the dDRM line when green.
