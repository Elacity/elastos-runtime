# Commerce — publish → index → buy → acquire → trade

The money rail: how a sealed asset becomes a listing, how a buyer gets an on-chain access
right, and how the encrypted file lands in their Library so the existing viewer can open
it.

Companion pages: [COMMERCE_API.md](COMMERCE_API.md) (the `/api/market/*` seam),
[COMMERCE_CONTRACTS.md](COMMERCE_CONTRACTS.md) (selectors, addresses, ABIs on Base).

---

## 1. Scope — what the marketplace is, and is not

**The marketplace is the single place you go to discover, buy, trade, list, and withdraw
dDRM assets.** It **mints nothing, plays nothing, renders nothing, and holds no signer,
token, CEK, IV, or RPC.** It produces two things and stops:

1. an on-chain **access right** — an unsigned `buyAccess` your wallet signs and broadcasts;
2. the bought **encrypted file pinned into your local Library** — managed like any other file.

The existing **viewer** opens it. The existing **creator app** mints it.

### One storefront, all asset types

| Asset type | Discover / buy / trade / list | Fulfillment on buy |
|---|---|---|
| **Content** (watch · listen · read · view · explore) | unified | pin the encrypted file into the Library, open in the runtime's viewer |
| **App / game** (future) | unified | **install** via the existing app-store path (capability grant plus consent) |

Discovery, the trade verbs, royalties, and the access-right model are identical across
types, so the storefront unifies. The privileged "install = grant capabilities" step stays
in the existing app-store machinery, invoked only as the app/game *fulfillment*, so
capability hygiene holds and there is one canonical path. Content is implemented; apps and
games are scaffolded in the information architecture.

### Explicitly out of scope (it lives elsewhere in the runtime)

| Concern | Owner |
|---|---|
| Minting, encrypt, KID seal, content upload | the `creator` capsule. Sale terms are baked into the mint, so **primary listing == minting** — not a marketplace step |
| Playback, rendering, decrypt, CEK, IV, key release | `elacity-player`, `ddrm-viewer`, `decrypt-provider`, `key-provider` |
| Viewer selection | the runtime, from the sealed object's `viewer.required_interface` |
| The `drm/open` sequence | planned by `drm-provider`, executed by `ddrm-plan-runner` — never called by the marketplace |
| Raw content-network access | capsules use the `content/*` plane; the marketplace never calls `ipfs-provider` directly |
| In-app "secure preview" egress | removed. Asset detail shows a **poster/cover only** (optionally a pre-rendered public teaser produced at encode time that needs no decrypt-provider). No new free-scope secure-preview trust boundary |

### Two consolidation items, decided but unbuilt

- **One canonical Marketplace.** `capsules/marketplace` (today's app-store) and
  `capsules/marketplace-content` (the content storefront) must consolidate to one
  asset-type-aware surface, with the app-store folded in as the "Apps" fulfillment. Do not
  maintain two storefronts.
- **Launcher wiring** for that canonical surface.

---

## 2. Publish — the producer side

`encrypt-provider` seals the asset (see [MEDIA_PIPELINE.md](MEDIA_PIPELINE.md)); the CEK is
escrowed to the dKMS quorum at **publish** time, to the authority's stable published
recipient. Then:

- `publish-provider` emits an `UnsignedMintV1` intent.
- `chain-provider.assemble_mint` ABI-encodes `mint(string _uri, uint16 opType, bytes
  opRawData, bytes sellRawData)`, with `contentId == bytes16 KID` carried in `opRawData`.
- `wallet-provider` signs (the key never leaves the capsule); `chain-provider` broadcasts.
- `POST /api/create/mint` (`mint_authority`) is the gateway surface. It is a **money verb**
  — see §4.

`opType ∈ {FREE: 0, BUY_ONCE: 1, BUY_AND_RESELL: 2}`; roles `{ACCESS_TOKEN: 1,
ROYALTY_SHARE: 2, DISTRIBUTION_RIGHT: 3}`.

One identity survives every hop — the KID is the `contentId` in the mint calldata and the
`content_id` of the resulting listing. `scripts/ddrm-market-smoke.sh` and
`scripts/ddrm-publish-smoke.sh` prove that cross-binary.

---

## 3. Discovery — the content index

**The index is not a provider capsule.** It is a server-side cache table inside the gateway
(`api/content_index.rs` + `api/gateway_marketplace.rs`), fed by a polling job that calls
`content-market` (the pure calldata/event decoder) over `chain-provider` (the sole RPC
declarant). It holds no keys, no RPC of its own, and no write authority — it is a query
accelerator *below* the canonical calldata path, never a trusted oracle.

**Polling, not subscription.** `chain-provider` exposes `eth_getLogs` only, capped to
10 000-block windows on a curated RPC subset. The snapshot carries persistent cursors
(`scanned_to` / `backfill_low`). Each advance cycle runs two lanes:

- a **delta lane** — head tracking, 120-block reorg overlap, at most 16 windows per cycle;
- a **backfill lane** — `ELASTOS_MARKET_BACKFILL_WINDOWS` per cycle (default 8), working
  back toward the EventHub deploy block.

A per-process poll loop (`ELASTOS_MARKET_POLL_SECS`, default 300 s) advances even while
idle. `GET /api/market/search` reports honest `coverage` (`recent-window` → `indexing` →
`indexed`) and caps results at 200, newest first.

**Enrichment.** The lean listing (operative, token id, op type, token URI) becomes a rich
one via `content-market`'s `enrich_listing` op, which is **pure** — the caller hands it
everything. The orchestration is: index row → fetch the mint calldata
(`chain-provider tx_input`) → fetch `metadata.json` by CID through the `content/*` plane →
fuse. `enrich_listing` re-derives the `contentId` from the calldata (authoritative) and
**requires `metadata.kid == contentId`**, else `identity_mismatch` — a metadata that lies
about its KID fails closed. Output carries `name`, `description`, `image_url`,
**`content_cid`** (the encrypted asset CID the buy→pin needs), `mime_type`, `asset_type`,
`creator_address`. Enrich lazily on detail views, not per discovery card.

**Do not trust inline `metadata.pricing{}`.** It is a legacy v1.0 field; authoritative
price, supply, and op type come from the on-chain Operative.

**Honest limits, carried in code and docs:** freshness is bounded by polling — state the
SLO, never imply real-time. The cache is centralized-but-verifiable: the same trust *shape*
as a subgraph, with the guarantee being re-derivability plus live re-verification at point
of use, not "no chokepoint". Metadata enrichment may lag or fail; rows persist with an
unresolved status and identity plus sell terms survive from calldata.

---

## 4. Buy — the money-path invariant

### The bug this exists to prevent

The worst failure a marketplace can have: a byte-correct `buyAccess` transaction is signed
and broadcast against a **fabricated token id or a stale price**. The wallet debits, the
transaction confirms with on-chain finality and no refund, and
`has_access_by_content_id` for the asset stays **false** — so the open fails closed.
**Paid, no access, no refund.**

The original defect was env-pinned terms plus a `tokenId` derived from a hash of the
content id rather than the real on-chain ledger token id.

### The invariant

Between "user confirms buy" and "assemble the unsigned transaction", insert a
**re-verify-then-bind** step, and abort on drift:

1. **Re-decode the listing from chain** (not the index cache) via `content-market`
   `reconstruct_listing` / `listing_from_event`, over `chain-provider`.
2. **Re-read live on-chain state** — the real `tokenId`, current `price`, `payToken`,
   remaining supply, and the listing `seller`, bound to the asset's on-chain seller (never
   a caller-supplied or env value).
3. **Bind those exact values** into the unsigned purchase.
4. **Abort on drift.** Immediately before signing, re-read the same fields once more; if
   `(seller, tokenId, price, payToken, supply)` differ from what was bound at assembly,
   **fail closed — do not broadcast.**

### What the assembled call must bind

| Field | Correct source | The trap |
|---|---|---|
| `tokenId` | the **ledger media token id**, resolved from the KID via the `chain-provider` resolver (`AssetCreated` plus mint calldata), **fail-closed on ambiguity** | never a hash of the content id |
| `ledger` | the channel's ERC-1155 ledger (`metadata.properties.ledger`) | **not** the gateway |
| `seller` | the on-chain listing seller from `sellersOf` / `listings` | not a caller or env value |
| `pricePerToken` | the listing price in pay-token minor units (USDC = 6 decimals) | read live, lowest active seller |
| ERC-20 `approve` spender | the asset's Operative `paymentProcessor()`, read live | **not** the gateway |
| native `value` | `pricePerToken × quantity` | not a flat price |
| `authority` (the gateway) | per-asset, from `metadata.properties.authority` | do not hard-pin |

**`listings` and `sellersOf` are keyed at ACCESS_TOKEN id = 1**, not the content token id —
`buyAccess` keeps the content token id. Reading at the content token id returns an empty
slot, which makes every live buy abort as drift or sold-out and shows no price on detail.
Pinned by `listings_read_is_keyed_at_access_token_id_one_not_content_tokenid`.

Do not add chain RPC to `buy_authority` — go through `chain-provider`, the sole RPC
declarant.

### The signing posture — unsigned to an external wallet

The buy stays **UNSIGNED** and is routed to the user's own wallet. The managed-account
autosign mode (`ELASTOS_DDRM_BUY_SIGN=wallet`) is compiled **`dev-modes` only**; a release
build never self-signs and hands back the unsigned transaction (409) for an external
wallet. This is a hard gate enforced in code, not just documented.

### The money-verb gate

`POST /api/market/buy` and `POST /api/create/mint` are the two **node-signed money verbs**.
Both pass `authorize_money_verb` (`api/viewer_open.rs`), which requires:

- a **Home-hosted, proof-bound launch** presented as the `home-session` cookie
  (`HttpOnly; SameSite=Strict; Path=/`, browser-origin-pinned to the `home` capsule); and
- a **fresh, single-use passkey step-up** bound to the launch *and* to this exact intent —
  the request body verbatim minus `step_up_token`, so altering any term (asset, seller,
  quantity, price, pay token, mint destination) after the ceremony rejects the replay.
  Maximum age: 180 s, identical to the Wallet and recovery step-up window.

A standing Home session is authentication, not authorization to spend. Every refusal is a
**403** drawn from a closed set of three messages — not signed in, step-up required,
step-up rejected — so an unauthorized spend is never a status a caller can retry into a
spend, and the node's internals are never narrated to the caller.

Home brokers the whole verb itself (`capsules/home/browser/home-shell-host.js`), and shows
its own spend confirmation first (`home-spend-prompt.js` — "Confirm purchase" / "Confirm
mint", every field of the intent rendered as text, escape cancels, one spend at a time)
**before** the ceremony, so a declined spend never touches the authenticator. The browser
side relies on the `SameSite=Strict` cookie and deliberately does not send the
`x-elastos-home-token` header. Only the storefront capsule and the Home GUI shell may
request a money verb.

The `/api/market/order/*` and `/api/market/acquire` routes are **assemblers and
fulfillment**, not node-signed spends: they are gated by a Home launch token scoped to the
`home`, `marketplace`, or `marketplace-content` capsules, and `acquire` additionally gates
on a live on-chain entitlement check.

---

## 5. Acquire — buy → pin to Library

The one genuinely new runtime seam. On a confirmed buy, the bought **encrypted** asset is
pinned into the buyer's local Library and registered as a `LibraryObject`, so the existing
viewer opens it.

`ObjectProviderRequest::Acquire { principal_id, content_cid, uri?, metadata? }` (the enum
is `deny_unknown_fields`, so unknown keys fail closed). `library_acquire`:

1. **Derives the destination under the buyer root only** — resolved via the principal's own
   localhost root, so a buyer can never pin into another principal's space.
2. **Pins through the `content/*` plane** (`content/ensure` → `local_pinned`), never raw
   content-network access.
3. **Fetches the bytes keylessly** (`content/fetch`). They stay opaque ciphertext.
4. **Materializes under the buyer root**, encrypt-at-rest if the root is protected.
5. Appends a Library event and returns the Library `uri` — the openable path the viewer
   resolves, still ciphertext until the rights/key/decrypt providers run at open.

Built with the lower-risk **additive** design: the acquired asset is a normal unpublished
Library file, with **no** change to the publish path or its gate. It holds no keys and
never decrypts. The registry-less standalone capsule path **rejects** `Acquire` — the buy
flow must target the in-process runtime path that has the registry.

**Entitlement is gated upstream** by the marketplace/buy caller: `POST /api/market/acquire`
verifies `hasAccessByContentId` before dispatching. Pinning encrypted bytes grants no
decryption — keys are gated at open — so the blast radius of a gap here is wasted storage,
not disclosure. The fetch is size-capped (`ELASTOS_DDRM_ACQUIRE_MAX_BYTES`).

**Hardening follow-on:** once the server resolves the canonical `content_cid` for a
`content_id` from the asset metadata (§3 enrichment), `market_acquire` should **ignore the
client-supplied `content_cid`** and pin only the canonical CID.

---

## 6. Resale and withdraw

Secondary sale of **owned** access. Same discipline as buy: pure, selector-pinned, unsigned
to the wallet, no keys and no RPC in the assembler (`api/trade_authority.rs`).

| Step | Call | Notes |
|---|---|---|
| Prerequisite | `setApprovalForAll(operator = AuthorityGateway, true)` `0xa22cb465` | an **Operative/ledger (ERC-1155)** method, not a gateway method — once per owner |
| List | `sellAccess(ledger, tokenId, quantity, pricePerToken, payToken)` `0x9a3fa9f5` | bind the reseller as seller; require a live `hasAccessByContentId(subject, KID) == true` proof before assembling |
| Cancel | `withdrawListing(operative, tokenId, quantity)` `0x3e65bbba` | the access right is unaffected |

Note the intentional asymmetry: `sellAccess` arg 0 is the **ledger**, `withdrawListing`
arg 0 is the **operative**; the gateway maps between them. Resolve the real ledger token id
from the KID with the same resolver the buy path uses.

A separate **TradeGateway** carries the secondary royalty-share market
(`sellToken` / `buyToken` / `createOffer`) — see
[COMMERCE_CONTRACTS.md](COMMERCE_CONTRACTS.md).

---

## 7. Open handoff — the marketplace renders nothing

For an owned asset, hand off to the existing open path: `POST /api/viewers/open { uri }`
with the Home launch token, optionally preceded by `POST /api/viewers/prepare-grant` plus a
wallet `personal_sign` for chain-mode dKMS. It returns
`{ viewer, session, title, play_url, rights_binding }`; open `play_url` as an iframe
targeting the returned viewer. The cleanest path is to emit a Library open launch and let
the Home shell run open (with the buy retry).

The marketplace must **not** build a session, fetch the media or object routes, call
`decrypt-provider`, or embed `<video>` / MSE. The runtime picks the viewer from the sealed
object's `required_interface`. See [VIEWER_SESSIONS.md](VIEWER_SESSIONS.md).

---

## 8. The agent-payment rail

There is a second way a buy happens: an AI agent spending under a signed mandate, through
`runtime.pay` with a DRM asset as the payee. That rail reuses the same
`buy_authority` resolve → quote → settle path under a durable spend meter, with
confirmation-aware settlement and a portable receipt. It is documented separately:
[../DRM_MARKETPLACE_RAIL.md](../DRM_MARKETPLACE_RAIL.md) (wiring, env, state model) and
[../LIVE_BUY_RUNBOOK.md](../LIVE_BUY_RUNBOOK.md) (the live-money last mile on testnet).

---

## 9. Verification

```bash
just test-crate elastos-server        # includes the wrong-token and drift regression tests
scripts/ddrm-market-smoke.sh          # publish → chain → listing, one identity across every hop
scripts/ddrm-publish-smoke.sh         # KID → contentId → byte-faithful mint calldata
node docs/dkms/tools/verify-selectors.mjs   # re-confirm selectors against deployed bytecode
node docs/dkms/tools/index-proto.mjs        # a working content-index against the live chain
```

Re-run `verify-selectors.mjs` after any proxy upgrade — both gateways are upgradeable.

---

## 10. Honest invariants — never overclaim

- **"Keys used, never owned."** The key custody is an operator-curated quorum, not "fully
  decentralized" and not "uncopyable".
- **The bought asset is the encrypted file.** Pinning it locally grants no decryption; keys
  are still gated at open by the rights → key → decrypt path.
- **The index is centralized-but-verifiable** — re-derivable and re-verified at point of
  use, not "no chokepoint".
- **The marketplace mints nothing, plays nothing, holds no keys.**
- **The pure on-chain model has no revocation or takedown story.** A real product and legal
  gap to own.

---

## 11. Where the design came from

The council plans, the UI-donor decision, and the phase-by-phase build specs are preserved
in [history/COMMERCE_PLAN.md](history/COMMERCE_PLAN.md) and
[history/COMMERCE_UI_STRATEGY.md](history/COMMERCE_UI_STRATEGY.md). Both predate parts of
the scope correction above; this page governs.
