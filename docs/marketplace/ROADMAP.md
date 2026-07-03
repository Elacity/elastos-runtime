# Marketplace-in-Runtime — Branch Roadmap & Status

> Status of `feat/marketplace-runtime` (off the dDRM line; merge back when green). For the Cursor handoff. The goal: a **buy/trade venue for all dDRM assets** — everything shows up and can be bought · traded · listed · withdrawn; on buy the encrypted file pins to your Library and opens in the existing player. **Scope (canonical): [SCOPE.md](SCOPE.md).** Specs: [PHASE1_BUY_INVARIANT.md](PHASE1_BUY_INVARIANT.md), [PHASE2_INDEX_AND_API.md](PHASE2_INDEX_AND_API.md), [PHASE3_ACQUIRE_AND_TRADE.md](PHASE3_ACQUIRE_AND_TRADE.md) (buy→pin · resale/withdraw · open-handoff). Contracts: [CONTRACTS.md](CONTRACTS.md). Live index reference: [index-proto.mjs](index-proto.mjs). ([PLAN.md](PLAN.md) predates the scope correction — see SCOPE.md.)

## Where we are — ~72% built end-to-end (backend ~85%; ≈95% designed + de-risked)

> **Autonomous build-loop output (this arc):** Phase-1 buy-invariant · KID→tokenId resolver · discovery (`search`/`sections`/**`get`**) · **`vault`** · the **object-provider `Acquire` op** (buy→pin) · the **entitlement-gated `/acquire` wrapper** · resale (`order/{sell,withdraw,approve}`) — all compile + unit-tested. The **shell is wired to the live gateway** (Home token · live-Listing normalize · `get`/`acquire`/`vault`/open handoff; browser-verified standalone) and the **`marketplace-content.wasm` launcher is built**. **Remaining is the live boundary:** the **enrichment** (name/cover/`content_cid`) ALREADY EXISTS as `content-market`'s `enrich_listing` op — wire `/get → content-market` (a LIVE `metadata.json` fetch via ipfs-provider) + the **live-chain test** + **launcher registration** are Cursor's. 50 commits.

### (historical) ~40% built (≈90% designed + de-risked)
**Scope corrected (this pass):** the marketplace is buy/trade ONLY — it **mints nothing** (creator app) and **plays nothing** (runtime players); on buy it pins the encrypted file to the Library and hands off opening to the player. The mint wizard + in-app secure-viewer were removed.
- ✅ **UI/UX** (discover · buy · trade · list · withdraw, asset-type aware) — built, **browser-verified**, USDC-correct; mint/playback removed.
- ✅ **Contract layer** — AuthorityGateway `0x09dBe7` + TradeGateway `0xd02451` + EventHub `0x5a694A6d`, all selectors/events live-verified; runtime == elacity == PC2 v3.
- ✅ **Discovery** — `index-proto.mjs` decodes 50 real Base listings (Phase-2 proven, not just specced).
- ✅ **Integration seams audited** — the player open-handoff (`POST /api/viewers/open`), the dDRM boundary, and the IPFS-pin path are grounded in real runtime code (SCOPE.md).
- 🔨 **Backend Rust — building (compile + unit-tested here; Cursor live-chain tests):** ✅ `trade_authority` (resale/withdraw/approval, 5/5) · ✅ `chain-provider` KID→tokenId resolver (AssetCreated+calldata) · ✅ **Phase-1 buy-invariant COMPLETE** in `buy_authority` (real tokenId, value×qty, approve via paymentProcessor, abort-on-drift, qty/price word order SSOT-corrected, all fail-closed; 7 active + 1 dev-modes + 3 live-ignored) · ✅ **content-index discovery slice** (`content_index` decode+cache+search/sections wired to `GET /api/market/search|sections` via a recent-window chain scan; 2/2). ✅ **resale routes** wired (`/api/market/order/{sell,withdraw,approve}`, `trade_authority` 5/5 in-crate). Next: content-index persistent cache + KID/metadata enrichment + `/api/market/get|vault`; the **`object-provider Acquire` op (buy→pin) — turnkey-specced** in [PHASE3_ACQUIRE.md](PHASE3_ACQUIRE.md), **Cursor-owned** (it *modifies* the publish path + its `content/ensure`+`fetch` core is live-registry-only); open-boundary (dKMS, external audit).
- ✅ **Security + PRINCIPLES + carrier audit pass** (15-seat council + 6-lens deep audit, this branch) — 11/13 confirmed findings fixed: the qty/price word-order money-path bug + non-hex decode panic; the KID→tokenId resolver now **fails closed on ambiguity** (a hostile co-channel minter can't bind the wrong tokenId); `/api/market/order/*` Home-token-gated + address-validated; the SCOPE unsigned→external-wallet **hard-gate enforced in code** (managed autosign is `dev-modes`-only) + seller fail-closed; `content_index` checked-arithmetic + length cap; clippy-clean; `esc()` hardened; discovery short-TTL cache. Deferred (land with the op): the Acquire entitlement gate + the `record_is_published` chokepoint.
- ✅ **Phase-2 `GET /api/market/get`** — live re-verified detail terms (`sellersOf` + `listings`, lowest active price) in new `market_reads.rs` (pure ABI, fail-soft) + reuses `read_listing_terms` (P10). Unblocks live shell wiring.
- ✅ **Phase-3 `object-provider Acquire` op — BUILT (additive)** ([PHASE3_ACQUIRE.md](PHASE3_ACQUIRE.md)) — buy→pin: fetch keylessly via `content/*` → `content/ensure` → materialize the encrypted asset under the buyer's Library root (no `record_is_published` change; entitlement gated upstream). Closes the buy→library→open loop; unblocks `/vault` (read the Library's Acquired assets). Live fetch/ensure + open = Cursor.
- ⏳ **Canonical surface** — one Marketplace for all assets (content now; apps/games later via the existing install path); wire into the launcher, fold in the app-store. **Decided; unbuilt.**
- ⏳ **Open-boundary** — threshold CEK release + dKMS v0 (largest item; external-audit dependency).

## The lifecycle, by layer
| Verb | UI (shell — built & runnable) | Backend (Cursor + real chain — specced) | Acceptance ("done" =) |
|---|---|---|---|
| **Show up / discover** | ✅ Discover · Search · facets · Asset-detail (poster/cover + commerce sidebar), asset-type aware, renders from `/api/market/*` (mock fallback) | ⏳ `content-index` polling cache + `/api/market/*` gateway routes (**Phase 2**) | a minted asset appears in `market/search` within the freshness SLO; every row re-derivable from calldata |
| **Buy** | ✅ buy flow + quantity stepper + optimistic pre-flight; assembles an **unsigned** order routed to wallet | ⏳ **Phase 1** buy-invariant (bind terms to the re-verified on-chain listing, abort on drift) + open-boundary | wrong-token buy can't broadcast; pay → `has_access` true → open works |
| **Trade / list (resale)** | ✅ "List for resale" + live royalty-net preview; unsigned → wallet | ⏳ resale order-assembly (reseller bound as seller, requires `has_access` proof) | owner lists → secondary listing with floor; another wallet buys; splits enforced on-chain |
| **Withdraw / cancel** | ✅ Vault "Listed" tab → Withdraw; unsigned cancel → wallet | ⏳ `cancelListing` order-assembly | listing removed on-chain; access right unaffected |
| **Acquire (buy→pin)** | ✅ buy-success reframed to "pinning to your library"; `API.acquire` trigger contract | ⏳ on-buy pin: `content/ensure` (→`local_pinned`) + a **new `object-provider` `Acquire` op** to register the encrypted CID as a `LibraryObject` | bought encrypted asset appears in the user's Library, managed like any other file |
| **Open / consume** | ✅ owned → "Open in your library" hands off; marketplace renders **nothing** | ⏳ runtime's existing path: `POST /api/viewers/open` → `elacity-player`/`ddrm-viewer` (rights→key→decrypt); + open-boundary dKMS v0 | `has_access` true → opens from the local Library in the existing player; raw bytes never egress |

## Done on the branch (16 commits)
1. `PLAN.md` + `PHASE1_BUY_INVARIANT.md` + `layouts.html` — corrected plan, the gate spec, the visual.
2. `capsules/marketplace-content/` — runnable shell capsule, **holds no authority (P16)**.
3. `PHASE2_INDEX_AND_API.md` — discovery layer spec.
4. Trade flows — buy pre-flight, resale, activity.
5. Design-system pass — tokens, motion, a11y.
6. **Lifecycle complete** — buy quantity, functional Vault tabs, withdraw/cancel.
7. **`CONTRACTS.md` + `verify-selectors.mjs`** — turnkey contract reference; every selector empirically confirmed against deployed Base bytecode (incl. EIP-1967 proxy impls).
8. **Contract reconciliation** — runtime == elacity == PC2 v3 (identical AuthorityGateway + TradeGateway); pay-token confirmed = canonical Base USDC (no WELA on Base — ESC only); EventHub + from_block banked.
9. **`index-proto.mjs`** — working content-index proving Phase-2 against the live chain (50 real listings decoded; EventHub emits `AssetCreated`).
10. **Pay-token corrected ELA→USDC** across the shell.
11. **Rescope audit** — orchestrated runtime audit (players/creator/IPFS-pinning/dDRM boundary) + adversarial verification → [SCOPE.md](SCOPE.md).
12. **Re-scoped to buy/trade ONLY** — removed the Studio mint wizard + the in-app secure-viewer; buy→pin-to-Library + open→player handoff; Vault = Owned/Listed/History; asset-type aware (Content; Apps/Games scaffolded). Browser-verified end-to-end.

**UI/UX for the corrected buy/trade lifecycle is built, browser-verified, and design-elevated.** Open `capsules/marketplace-content/browser/index.html` to review.

## Remaining to runtime standard (build order for Cursor)
1. **Phase 1 — buy-invariant** (the gate; money-path; **a NEW build item, not yet present** — `buy_authority.rs` uses env-pinned terms + `word_from_id(content_id)` tokenId). No buy ships before terms are re-read live + abort-on-drift AND a real `AssetCreated/DigitalAssetRegistered → ledger tokenId` resolver + Operative primary-price getter exist. **Unsigned→external-wallet only** (never the managed-account autosign mode). *Parallel:* **open-boundary** (threshold release + dKMS v0 — largest item, external-audit dependency).
2. **Phase 2 — `content-index` + `/api/market/*` routes** (the discovery cache; no UI dependency; turnkey via `index-proto.mjs` + CONTRACTS.md).
3. **Buy→pin (`Acquire`)** — on confirmed buy, `content/ensure` the encrypted CID + a **new `object-provider` `Acquire` op** to register it as a `LibraryObject` (the one genuinely new runtime seam; design proven in PC2, re-implement in Rust). Surface pin-status.
4. **Resale + withdraw assemblers** — UNSIGNED `sellAccess`/`withdrawListing` (selector-pinned, wallet-signed; `capsules/marketplace/src` empty today).
5. **Open handoff** — wire owned → `POST /api/viewers/open {uri}` (or a Library open launch); the runtime opens `elacity-player`/`ddrm-viewer`. No new render code.
6. **Wire the shell to live data** — flip `api.js` mock → real routes (mock stays under a `?mock=1` dev flag).
7. **Canonical surface + launcher wiring** — make this the one Marketplace for all assets; fold the existing app-store `capsules/marketplace` in as the "Apps" fulfillment; register in the home launcher (P10). Confirm the **Operative primary-price getter** + the **KID→ledger tokenId** resolver. Creator-identity / anti-sybil before scale.

## Deferred (deliberate, by direction)
- **Icons:** match the runtime's inline **feather/lucide-style stroke-SVG** convention (no installable pack exists; no scratch SVGs) — emoji are placeholders until then.
- **Apps & Games asset types** — scaffolded in the IA now; wired (install via the existing app-store path) when dDRM wraps executables.
- **Thin `marketplace-content.wasm` shell** — packaging follow-up.

## Honest invariants (never overclaim)
"Keys used, never owned" (operator-curated 2-of-3 dKMS — not "fully decentralized/uncopyable"). The bought asset is the **encrypted** file pinned to your Library — pinning it locally does **not** grant decryption; keys are still gated at open by the rights/key/decrypt path. The index is centralized-but-verifiable (re-derivable + re-verified at point-of-use), not "no chokepoint." The marketplace mints nothing, plays nothing, holds no keys. No buy button is real until Phase 1 lands.
