# Buying from any live offer — market buy design

Date: 2026-09-24. Status: **implemented on the branch; first installed
buy passed on Base mainnet on 25 September (see the
[implementation report](2026-09-25-protected-content-market-buy-implementation-report.md));
the remaining installed journeys are open.** Implementation lands on `feat/protected-content-listing-buy`, stacked
on PR #62.
Scope: how a buyer finds, buys and later opens a protected item in the
Marketplace. The custody ceremony, the rights request, the decrypt provider
and the contracts are unchanged.

This document is the reviewable statement of what changes in architecture,
behavior and protocol. `docs/PROTECTED_CONTENT.md` describes what the code
does today and carries the amendments listed under
[Protocol changes](#protocol-changes).

## Why

Today a buyer can buy only an item whose **portable listing package** they
hold: a `listing.json` the creator's Runtime publishes to IPFS and shares as
`elastos://<cid>` (specified in `dd926bd8` and `cbbabd5a`, built in `9e955e65`
and `3026992b`, 2026-08-28). Four problems follow.

1. **Mutable terms are frozen into an immutable object.** A seller changes
   price or quantity with `sellAccess` at any time; the package still states
   the terms at mint.
2. **A second seller cannot be expressed.** With a resellable item a buyer who
   holds access tokens can list some of them, so one item has several offers.
   A package has one seller.
3. **Items minted outside this Runtime cannot be bought.** An item minted on
   ela.city or through the SDK has no package, and five of its 27 required
   fields (`mint_id`, `content_id`, `publisher_profile_did`,
   `mint_transaction_hash`, `published_at`) are not recoverable from chain and
   the shared metadata.
4. **Buying runs the read path.** Before any money moves, the buy path
   refreshes a replication receipt, verifies the content object and rebuilds
   the custody draft. Those checks answer *"can this be opened?"*, not *"can
   this be bought?"* — the access token exists on chain regardless — and they
   refuse every item this Runtime did not mint before its offer is read.

The 2026-08-28 specification said so itself: *"A shared listing link is
sufficient for 0.7; global market discovery remains later work."* This design
is that later work.

## Principles

- **Buying and reading are two workflows with two datasets.** Buy needs the
  item, the seller and live chain terms. Read needs the protection material.
  Neither borrows the other's checks.
- **The chain is the only authority for price, ownership and access.** An
  index names items; it is never trusted for anything a purchase relies on.
- **Rebuildable by anyone from common data.** A listing can be reconstructed
  from a `tokenURI`, a content id (KID), or `(ledger, tokenId)`, plus the
  chain and the shared Elacity `metadata.json` — data every marketplace has.
  Nothing ElastOS-only is required.
- **Origin never decides.** An asset qualifies by its protection scheme, not
  by which application minted it.

## Facts this design stands on

From `v3-drm-protocol` (read, not modified):

| Fact | Where |
| --- | --- |
| An item is `(ledger, tokenId)`; `operative(ledger, tokenId)` names its operative | `AuthorityGateway.sol` |
| `ipReference(kid) → (channel, tokenId)` binds each KID to exactly one item; the store is `AuthorityGateway.cstore()` | `storage/IPTracker.sol` |
| Offers are keyed `(operative, ACCESS_TOKEN = 1, seller)`; `sellersOf(operative, 1)` lists every seller; `listings(operative, 1, seller)` returns `(quantity, pricePerToken, payToken)` | `AccessTradeModule.sol`, `Ecosystem.sol` |
| `sellAccess` creates or overwrites a seller's terms | `AuthorityGateway.sol` |
| `buyAccess` reverts when the buyer's price or pay token differs from the live listing, or the quantity exceeds it | `AccessTradeModule.sol` |
| `tokenURI(1)` on the operative is `<metadata folder>/<64-hex id>.json` | `operative/kind/OperativePrimitive.sol` |

From the Runtime at PR #62 (`43585914`):

| Fact | Where |
| --- | --- |
| The shared `metadata.json` carries `kid`, `media.uri` (content CID), `media.mimeType`, `asset.protections[0]` with the rights-policy, key-envelope, key-commitment and content identities, and `properties.publisher` (an EVM address) | `protected_content_elacity_metadata.rs` |
| `manifest.json` in the same folder is ElastOS-only; other marketplaces neither write nor read it | `protected_content_runtime.rs` (`RuntimePortableMetadata`) |
| `mint_id` is the hash of a rebuilt mint draft; `content_id` is a hash of the encrypted content identity — both computable | `mint_journal.rs`, `protected_content_runtime.rs` |
| The purchase builders use only offer fields of the package | `gateway_provider_proxy.rs` |
| Offer reads are corroborated across ≥2 RPC sources at the finalized block | `capsules/chain-provider/src/main.rs` |

## Architecture

```
          FIND                                BUY                                READ
  tokenURI | KID | (ledger, tokenId)   item + seller + agreed terms        after a completed buy
  (shared link or index row)                     │                                  │
            │                                     ▼                                  ▼
            ▼                          fresh offer read (chain)            adoption, from the shared
   binding check (chain + shared       differs → terms_changed              metadata.json: builds the
   metadata.json)                      same    → Wallet effect,             records the existing open
            │                                     settlement, access         path already consumes
            ▼                                     evidence                           │
      ListingObject  ─────────────────▶  market-purchase record  ─────────▶  availability receipt,
   item · asset · offers · access                                          draft rebuild, custody
                                                                           release (unchanged)
```

### Find: the `ListingObject`

The Runtime builds it; the Marketplace capsule only renders it. It is built
per item, on demand, when a buyer opens an item or presses Buy. Shelf cards
keep showing the index's price, labelled as the index's claim.

```json
{
  "schema": "elastos.marketplace.listing/v1",
  "item":   { "chain_namespace": "eip155:<id>", "network": "…", "ledger": "0x…",
              "token_id": "0x…", "operative": "0x…", "kid": "0x… | null" },
  "asset":  { "uri": "elastos://<metadata-folder-cid>", "title": "…", "description": "…",
              "cover_cid": "…", "category": "…", "mime_type": "…",
              "readability": "verified | unverified | foreign | unknown" },
  "offers": [ { "seller": "0x…", "quantity": "0x…", "price": "0x…",
                "pay_token": "0x…", "payment_processor": "0x…" } ],
  "access_state": "available | purchased | creator",
  "source": { "start": "token_uri | kid | item", "read_at_block": "0x…" }
}
```

**The binding check.** Whichever identifier starts the request, the Runtime
derives the other two and checks three legs (every KID compared here is
normalized first — R9):

1. `ipReference(kid) == (ledger, tokenId)`;
2. the folder of `tokenURI(operative, 1)` is the asset folder;
3. `metadata.json.kid == kid`.

The check is evidence for readability and adoption, not a purchase gate
(R46). A purchase rests on the chain's terms: the item, its non-zero
operative and its `tokenURI` folder. Only a proven contradiction refuses the
request as `asset_mismatch` (409): a `metadata.json` that was read and whose
KID binds on chain to a different item, or KID fields that disagree with each
other. When `metadata.json` cannot be read, or the KID-binding read is
unavailable, the Runtime still answers the `ListingObject` with its offers,
`asset.readability: "unknown"` and `item.kid: null`. Any body the endpoint
cannot parse — including one axum itself rejects before the handler runs —
answers `invalid_start` (400) (R34). A CIDv1 link to what is really a CIDv0
folder is refused rather than normalized across the two encodings: the check
fails closed instead of assuming an equivalence.

**Offers** come from the chain: `sellersOf` then `listings` for each seller,
at one finalized block — the lowest finalized block among the sources — and
corroborated (R32). Sold-out offers are dropped; at most 32 sellers are read
per item, and truncation is stated as `offers_truncated: true`. Every offer carries
`payment_processor`, JSON `null` for a native-token offer (R16).

**Readability is advisory.** It is decided from the shared document alone:
`verified` only for a protection entry of scheme
`cenc:elastos-pq-hybrid-threshold-v1` carrying all four identities as
non-empty canonical base64;
any other ElastOS-scheme entry — an older `-v0` entry, or a `-v1` entry
missing an identity — is `unverified`, not `foreign`; `foreign` is reserved
for a document with no ElastOS-scheme entry anywhere (for example a
Lit-protected mint, whose access token ela.city honours) (R12; see
[Rulings made during implementation](#rulings-made-during-implementation) for
the live fact this rests on). A document this Home cannot fetch, or whose KID
binding the chain cannot confirm now, is `unknown` (R46): Marketplace keeps
Buy enabled, says nothing about where the item opens, and treats a catalog
row as not learned yet. A fetched document that is not valid JSON answers
`asset_mismatch` (409). No readability value blocks a purchase.

**Access state** is computed by the Runtime for the principal's account, read
by KID; when the chain cannot answer, it is `available` with
`access_unknown: true`, never a claimed ownership (R17). The page never
receives the account address.

### Buy: `buy_offer`

A buy request names the item, the asset URI, the seller and the **terms the
buyer agreed to** (price, pay token, quantity `0x1`).

1. The item is re-derived from the chain alone (R46): `(ledger, tokenId)`
   gives a non-zero operative, and `tokenURI` gives the asset folder. The
   request's `asset_uri`, chain namespace and network must equal what the
   chain gives. `buy_offer` reads no `metadata.json` and no KID binding; a
   `kid` the page sends does not gate the purchase. Nothing the request
   asserts is trusted without being re-derived.
2. The seller's offer is re-read. If price or pay token differ, or the offer
   is sold out or gone, the answer is **`terms_changed`** with the current
   terms, and **no Wallet request is raised**.
3. Otherwise the existing purchase machinery runs unchanged in substance:
   optional ERC-20 approval, the `buyAccess` effect, settlement, then access
   evidence from `hasAccessByContentId` with the verified KID.
4. If the buy transaction is observed mined-reverted, or the Wallet declines
   the effect, the attempt is retired — its record removed — rather than
   kept for resumption; on a mined revert the offer is then re-read, and
   changed terms are answered as `terms_changed`, anything else as today's
   failure. A revert known only from the node's error text proves nothing
   is spent: that attempt is kept and the answer is the plain failure. The next press for that item starts a fresh attempt and a fresh
   effect: a random per-attempt `attempt_id` carried in the record (and
   bound into the purchase request hash) keeps it distinct from the retired
   one even at unchanged terms.

A press naming a **different seller or different agreed terms** than a
recorded attempt already in flight does not drive that attempt: it answers
`attempt_in_progress`, naming the recorded seller and terms, and raises no
Wallet request. Once the recorded attempt's buy is confirmed, the money has
moved, and any press finishes that attempt whatever terms it names. Creating a new attempt for an item is create-only, under a
per-principal lock, so two concurrent first presses converge on one attempt
rather than raising two effects; retiring an attempt and persisting its
stage are each conditioned on matching `attempt_id`, so a stale request
never retires or overwrites a newer attempt. A buyer holds at most one open
purchase per item across both purchase paths: `buy_offer` refuses before
creating an attempt when a listing-package purchase for the same item is
unresolved (`already_owned` once it is `Complete`, otherwise
`attempt_in_progress` with `current: null`), and `buy` refuses symmetrically
against a market purchase (`already_owned` once it is complete, otherwise
`attempt_in_progress` naming the market attempt's offer). Both checks run
again under the same market lock when the new record is written.

The terminal success answer is exactly the
`elastos.marketplace.buy-offer-complete/v1` schema in R11 — the capsule
parser recognizes only that shape as complete; anything else it does not
recognize is treated as failed.

**No availability receipt, content verification or draft rebuild runs during
a purchase.** Those are read-side checks and run at open.

Re-issuing the same request resumes the recorded attempt against the same
effect, on the terms recorded when it started — a reload or a second press
never pays twice. Other answers: `own_offer` (the seller is the buyer,
evaluated only once that seller is confirmed to hold a live offer),
`already_owned`, `asset_mismatch`.

A market purchase is stored in its own owner-only record,
`elastos.library.market-purchase/v1`, keyed by `(chain id, ledger, tokenId)`
and holding buy-side facts only.

### Read after a buy: adoption

Opening reuses the existing open path without modification. After a
completed purchase, an adoption step builds the two records that path
already consumes:

1. Read the shared `metadata.json` (capped at 256 KiB, R30), resolve its KID
   and check it against the chain binding (R46). When the KID cannot be
   resolved or checked, adoption stays `pending`; a proven mismatch writes
   no mint and is logged. No ElastOS protection entry → the item is
   `foreign`; the purchase stays complete and the card says it opens on
   ela.city. An ElastOS entry that cannot be adopted (a `-v0` entry, or a
   `-v1` entry missing an identity) leaves adoption `pending`.
2. Build a listing package from the shared document and the purchased offer.
   `content_id` is computed; the publisher is
   `did:pkh:eip155:<chainId>:<properties.publisher>`.
3. Run the existing content verification: fresh availability receipt, content
   object checked against its identity, custody draft rebuilt. `mint_id` is
   taken from that rebuilt draft, never read from anywhere.
4. Persist the listing record (new origin `Asset`) and a purchase record
   (new acquisition `market`), and file the owned `.ddrm`.

A failure in steps 1–4 leaves the purchase complete with adoption
`pending`. Adoption runs again each time this Home answers the completed
purchase: a repeated `buy_offer` for the item, or a request for its owned
copy. Nothing reruns it on a timer or at open. Marketplace therefore gives a
bought item whose adoption is `pending` a Download copy control, which asks
for the owned copy by its item (`download_owned_copy` with `{item}`) and
never pays (R45).

## Behavior changes a person sees

| Before | After |
| --- | --- |
| Only items shared by `listing.json` link could be bought | Any item found in Explore or reached by link can be bought |
| One seller per item | Every live seller is shown; the buyer picks one |
| A changed price produced a generic "purchase denied" | The new terms are shown and the buyer decides again |
| Buying an item whose file was not currently replicated failed | Buying succeeds; the card states whether the item can be opened here |
| ela.city-minted items were display-only | They can be bought; the card says they open on ela.city unless they use the ElastOS scheme |
| An item whose metadata this Home could not read could not be bought | It can be bought; the sheet says nothing about where it opens (R46) |
| A Buy that could not reach the market read "The purchase did not finish." | It reads "The market couldn’t be reached. Try again." (R48) |

## Protocol changes

**Contract text (`docs/PROTECTED_CONTENT.md`), amended when implemented:**

- Step 7 today: *"Marketplace reads bounded immutable listings. A buy request
  contains only the mint identity."* → *Marketplace reads listing objects
  built by Runtime from the chain and the asset's shared metadata. A buy
  request names the item, the seller and the terms the buyer agreed to;
  Runtime re-reads those terms and answers `terms_changed` rather than
  proceeding when they differ.*
- Step 8 today applies to **buy or open** → applies to **open**, and to a
  buy of an imported listing package (`buy`) only; `buy_offer` never runs
  it.
- *"Global listing discovery … separate later work"* → points here.

**New wire surfaces:**

| Surface | Kind |
| --- | --- |
| `POST /api/apps/marketplace/listing` — body `{"start": {"token_uri"\|"kid"\|"item": {ledger, token_id}}}`, answers `elastos.marketplace.listing/v1`, `{"legacy_listing": true}`, 409 `asset_mismatch`, 404 `unbound`, 503 `unavailable` when the offers cannot be read (R46), or 400 `invalid_start` for any body the endpoint cannot parse, including one axum itself rejects (R34) | gateway route (Marketplace capsule) |
| `buy_offer` — terminal answer `elastos.marketplace.buy-offer-complete/v1` (R11) | object-provider operation, Marketplace capsule only |
| `ResolveProtectedContentItem { network, ledger, token_id }` → `elastos.chain.protected-content-item/v1` | chain-provider op |
| `ResolveProtectedContentKidBinding { network, content_access_id }` → `elastos.chain.protected-content-kid-binding/v1` | chain-provider op |
| `ResolveProtectedContentItemOffers { network, ledger, token_id }` → `elastos.chain.protected-content-item-offers/v1` | chain-provider op |
| `elastos.library.market-purchase/v1` | on-disk record |

New contract selectors read: `sellersOf(address,uint256)` `0x997eab2d`,
`cstore()` `0xd36f509d`, `ipReference(bytes16)` `0x93d9f5ab`,
`tokenURI(uint256)` `0xc87b56dd`.

**Changed existing shapes, with compatibility:**

| Change | Compatibility |
| --- | --- |
| `RuntimePortableListingPackage.mint_transaction_hash` and `published_at` become optional | Absent **only** on an adopted (`Asset`) record. Every existing package serializes to identical bytes, so listing CIDs and package digests are unchanged. The listing import still requires both. |
| Listing origin gains `Asset { asset_uri }`; acquisition gains `market` | Additive; existing records decode unchanged |
| Package publisher accepts `did:pkh:eip155:…` | On `Asset` records only; every other origin still requires `did:key` |
| Purchase request hash gains a market variant (`elastos.protected-content.market-purchase-request/v1`), binding `asset_uri` plus a per-attempt random `attempt_id` | The listing-package variant stays byte-identical, so in-flight purchases resume; `attempt_id` lives only in the market purchase record and the request hash — it is never sent to Wallet as effect metadata |

**Unchanged:** the listing import and `listing.json` links (still accepted),
the open, read and close operations, custody release, the rights request, the
decrypt provider, and every contract.

## Security considerations

- **Trust boundary.** The index (today the Elacity GraphQL endpoint, used as
  an authorized external source under the existing consent, audit and
  partial-answer rules) contributes an identifier and display text only. Item
  identity comes from the three-leg binding check; terms and access come from
  the chain.
- **Terms.** The buyer's agreed terms travel in the request and into the
  transaction; the contract refuses a mismatch. Offer reads lag the head by
  the finalization delay (roughly 16–26 minutes on Base), so a very recent
  change can surface as `terms_changed` or a reverted transaction — never as
  a charge at terms the buyer did not see.
- **Advisory readability** means a buyer can knowingly buy an access token
  this Home cannot open; the card states it before purchase.
- **Publisher binding.** An adopted (`Asset`) listing names its publisher
  as `did:pkh:eip155:<chainId>:<properties.publisher>`, taken from the shared
  document. That `did:pkh` names the publisher and authorizes nothing (R40):
  it is not a claim of authorship, and it is never compared with the content
  manifest's publisher, which names the Runtime profile that published the
  bytes in another namespace. For a buyer-side availability refresh, the
  receipt must agree with the request on that publisher, and the bytes are
  bound by the content identity the document carries. The relaxation is
  explicit and narrow: `manifest.publisher_did` must still parse as a
  `did:key` (never merely non-empty text), and it applies only to
  `Asset`-origin listings, chosen by the caller from the listing's origin and
  never inferred from the requirement's own DID method.
- **Scheme conformance.** An asset that declares the ElastOS scheme but whose
  encrypted content does not follow the scheme's content-object layout reads
  as `verified` and fails at open. The open-time check is not relaxed; the
  requirement belongs to any producer of the scheme.
- **Privacy.** The page never learns the principal's account address;
  ownership reaches it as `access_state`.

## Out of scope

Subscriptions; buying more than one access token at once; listing or
withdrawing offers from ElastOS; moving Lit-protected assets under ElastOS
custody; the locked renderer; reading offers at head (recorded as an open
question).

## Later: Marketplace discovery

The GraphQL index is a workaround. Its replacement feeds the same entry
point:

```
today:  Elacity GraphQL (authorized index) ─┐
                                            ├─▶ ListingObject ─▶ buy_offer
later:  Marketplace discovery ──────────────┘
```

Any source that yields a common identifier can be swapped in without touching
the purchase, adoption or the offer sheet. The proposed direction is a
Runtime-native discovery provider that rebuilds its index from events the
protocol already emits (`AssetCreated`, `ItemListed`, `ItemSold`), so that no
operator's database is authority, with signed index snapshots optionally
exchanged between Homes as hints that are re-verified per item. ela.city
remains a public storefront over the same assets and offers; since both read
the same metadata folder and the same chain offers, the two agree without
sharing a database.

## Rulings made during implementation

Binding, made on `feat/protected-content-listing-buy` while this design was
implemented; each supersedes any conflicting sentence elsewhere in this
document, which have been updated to match.

- **R9 KID normalization.** Every KID read from a document is normalized
  before any comparison — strip an optional `0x`, require exactly 32 hex
  chars, lowercase, re-prefix `0x`. Lookup order: top-level `kid`, then
  `properties.kid`, then `asset.kid`. ela.city/Lit mints publish a bare-hex
  top-level `kid`, with no `0x`.
- **R11 `buy_offer` completion.** The terminal success answer is exactly
  `{"schema":"elastos.marketplace.buy-offer-complete/v1","item":{…},
  "asset_uri":"elastos://<cid>","adoption":"adopted"|"foreign"|"pending"}`.
  The capsule parser
  (`capsules/marketplace/browser/src/market-listing.js`,
  `parseBuyOfferAnswer`) recognizes exactly this shape as complete; anything
  unrecognized is failed.
- **R12 Readability.** `verified` only for a protection entry with
  `protectionType` exactly `cenc:elastos-pq-hybrid-threshold-v1` carrying all
  four identity fields as non-empty canonical base64; any other ElastOS-scheme entry (a `-v0`
  entry, or a `-v1` entry missing an identity) is `unverified`, not
  `foreign`; no ElastOS-scheme entry anywhere
  (`asset.protections[*].protectionType` or `media.protectionType[*]`) is
  `foreign`. A document that cannot be fetched is `unknown` (R46; this
  replaced the earlier 503 `unavailable`); a fetched document that
  is not valid JSON answers `asset_mismatch` (409) instead — PS-M6 corrects
  an earlier, unreachable clause that had called that case `unverified`.
  **Live fact:** ElastOS mints from before 2026-09-22 carry either `-v0`
  entries (the DASH era) or `-v1` entries without the four identities, so
  they read `unverified`, not `verified`.
- **R15 Listing endpoint failure modes.** *Superseded by R46 for an
  unreadable `metadata.json`.* An unreadable `metadata.json`
  answers 503 `unavailable` (the binding check's third leg cannot be
  checked, so it cannot be asserted as a mismatch either). A document whose
  own KID fields disagree with each other answers 409 `asset_mismatch`. A
  malformed `start`, or one that names nothing (for example a KID that is
  not 32 hex digits), answers 400 `invalid_start`. A folder with no
  `metadata.json` but a `listing.json` answers `{"legacy_listing": true}`
  for the existing import.
- **R16 Payment processor.** Every offer carries `payment_processor`; it is
  JSON `null` for a native-token offer.
- **R17 Access state under an unknown chain answer.** Access for the listing
  object is read by KID; when the chain cannot answer, `access_state` is
  `available` with `access_unknown: true` — never a claimed ownership.
- **R20 `buy_offer` wire shape.** The request matches the shared listing
  shape exactly — an item without `operative`, an optional `kid`, a
  top-level `asset_uri` and `seller`, and
  `agreed {price, pay_token, quantity}` — carried by
  a dedicated `deny_unknown_fields` struct. `operative` and `kid` are always
  derived by the binding check, never trusted from the request; a named
  `kid`, the `asset_uri`, and the chain namespace and network must equal
  the derived ones, or the answer is `asset_mismatch` (the `kid` clause is
  superseded by R46: a named `kid` does not gate). An end-to-end test
  exercises the page's real request body.
- **R21/R22 Retirement on a mined revert.** A buy attempt whose transaction
  is observed reverted is retired (its record removed) so the next press
  starts fresh; an attempt with a pending or unbroadcast Wallet effect keeps
  resuming, because the contract still enforces the agreed terms either way.
  Revert detection trusts the receipt status field (`0x0`) as authoritative.
  A revert known only from a text match on the node's error keeps the
  attempt and answers the plain failure, never `terms_changed`, because
  nothing proves its effect is spent.
- **R23 Per-attempt `attempt_id`.** The market purchase-request hash carries
  a random `attempt_id` stored in the record, so a retired attempt's
  successor gets a fresh effect even at unchanged terms, instead of
  resuming a dead one.
- **R25 Retirement on a decline.** An attempt the Wallet declines is retired
  the same way a mined revert is, so the next press starts fresh; a
  declined effect is never broadcast, so nothing is at risk.
- **R26 Create-only, compare-by-attempt.** Creating a new attempt for an
  item happens once, under a per-principal lock, and is create-only — a
  losing concurrent first press resumes the attempt the winner created
  rather than raising a second effect. Retiring an attempt and persisting
  its stage are each conditioned on a matching `attempt_id`, so a stale
  request never retires or overwrites a newer attempt.
- **R28 `attempt_in_progress`.** A `buy_offer` request naming a different
  seller or different agreed terms than a recorded attempt already in
  flight does not drive that attempt: it answers `attempt_in_progress`,
  naming the recorded offer, and raises no Wallet request. Terms compare by
  seller, price and pay token. Once the recorded attempt's buy is confirmed,
  a press on any terms finishes that attempt.
- **R29 One purchase per item, both paths.** A buyer holds at most one open
  purchase per item across the two purchase paths. Before creating a new
  attempt, `buy_offer` refuses when a listing-package purchase for the same
  item is unresolved (`already_owned` once it is complete, otherwise
  `attempt_in_progress` with `current: null`), and `buy` refuses
  symmetrically against a market purchase (`already_owned` or
  `attempt_in_progress` naming the market attempt's offer). Each path checks
  again under the same market lock before it writes its record, so two
  concurrent creates cannot both succeed.
- **R30 Bounded document reads.** The listing's reads of `metadata.json`
  and `listing.json`, and adoption's read of `metadata.json`, ask the
  content plane for a byte range, so it answers at most 256 KiB plus one
  byte; a larger document is refused (`unavailable` for the listing,
  adoption `pending` after a buy). The IPFS provider's `cat` still
  reads the whole file before the range is applied, so a true cap needs a
  provider change (TASKS.md follow-up). `listing.json` is probed only when
  `metadata.json` answers not-found, which is detected from the provider's
  error text until providers carry a typed not-found code.
- **R31 No address in Marketplace-facing errors.** Every `0x` followed by
  40 or more hex digits in a `buy`/`buy_offer` answer's `message` and
  `detail` is redacted to `0x…` before the answer leaves the Runtime; the
  typed `code`, `current` and `buy_progress` stay intact.
- **R32 Finalized pin for the new chain reads.** The three new
  chain-provider reads (item, KID binding, item offers) pin every source to
  the lowest finalized block among the sources — the discipline the rights
  read already used — instead of requiring every source to agree on one
  block.
- **R33 `own_offer` only for a confirmed seller.** `own_offer` is evaluated
  only after the named seller is confirmed to hold a live offer, so the
  answer cannot be used to test whether an arbitrary address is the seller.
- **R34 Any unparseable body → `invalid_start`.** A listing-endpoint
  request body the server cannot parse — including one axum itself rejects
  before the handler runs — answers 400 `invalid_start`, not a bare parser
  error.
- **R35 ERC-20 `current` without a processor.** For an ERC-20 offer with
  no payment processor recorded, `current` in `terms_changed` and
  `attempt_in_progress` is `null`, not a fabricated offer. The listing
  object refuses such an offer as a malformed chain answer (503
  `unavailable`).
- **R36 Typed chain answers.** The three new chain-provider answers
  (`ResolvedProtectedContentItem`, `...KidBinding`, `...ItemOffers`) are
  typed `deny_unknown_fields` structs, not ad hoc `Value::get` reads, and
  each answer's schema, network and chain id are checked against the
  request (the offers answer also its ledger and token id); the
  chain-provider writes a producer fixture that the server's tests
  deserialize.
- **R39 Equal-terms existing listing.** When a listing already exists for
  the same mint and its commercial fields (seller, price, pay token,
  processor, ledger, token id, operative, chain namespace/network, KID)
  equal the market purchase's, adoption binds the purchase record to that
  listing's existing package digest instead of leaving it pending. A
  listing with genuinely different terms for the same mint is a recorded
  limitation: adoption stays `pending` (TASKS.md follow-up). Quantity is
  availability, not a term, and is not compared.
- **R40 did:pkh publisher tightened.** The did:pkh relaxation requires
  `manifest.publisher_did` to parse as a `did:key`, not merely be
  non-empty; it is granted only as an explicit parameter passed by
  Asset-origin callers, never inferred from the requirement's DID method.
  The did:pkh publisher names the publisher and authorizes nothing.
- **R41 A listing attempt over without payment is retired.** A
  listing-package `buy` attempt whose Wallet approval was declined, or whose
  buy was mined and reverted (receipt status `0x0`), is retired: its
  purchase record is removed under the same per-principal market lock R29
  uses, and only while the record on disk is still that attempt, so a newer
  attempt is never removed. A declined Library buy then no longer blocks a
  `buy_offer` for the same item. A revert known only from the node's text
  keeps the record (R22).
- **R42 Redact before truncating.** `detail` is redacted of every `0x`
  followed by 40 or more hex digits before it is cut to 1024 bytes, so a cut
  can never leave part of an address behind.
- **R43 Ledger scan errors.** R29's scan of the listing-purchase ledger
  skips, with a warning, only a record that fails to parse or validate. An
  I/O error reading the ledger is returned, and the buy is refused as
  `unavailable`.
- **R44 A recorded attempt stays resumable.** When `attempt_in_progress`
  names the recorded offer (`current` is not `null`), Marketplace shows a
  Continue row for that seller on the recorded terms, even when the seller
  no longer lists the item or lists it on other terms. Only a press on
  exactly those terms moves the attempt on. With `current: null` the sheet
  shows a plain in-progress note, because no resumable offer exists for it
  (R35).
- **R45 A pending adoption has a control that never pays.** A completed
  market purchase whose adoption is `pending` shows Download copy on its
  catalog row ("Owned — preparing") and on its offer sheet ("You own this",
  readability `verified` or `unverified`). The control calls
  `download_owned_copy` with `{item}`, which reruns adoption and never pays,
  and then reloads the Library and the catalog. The completion message names
  that control instead of promising the item arrives by itself. No new wire
  operation.
- **R46 A purchase rests on the chain's terms only.** Owner rule: content
  availability, or proof that the CEK can be recovered, does not block a
  purchase. `buy_offer` fetches no `metadata.json` and runs no KID-binding
  read; it verifies the item from the chain alone — `(ledger, tokenId)` to a
  non-zero operative, `tokenURI` to the asset folder and `asset_uri` — and
  requires the request's `asset_uri`, chain namespace and network to match.
  A `kid` the page sends does not gate. The listing endpoint answers an
  unreadable `metadata.json` or an unavailable KID-binding read with the
  `ListingObject`, its offers, `asset.readability: "unknown"` and
  `item.kid: null`. Only a proven contradiction stays 409 `asset_mismatch`;
  an item not found or with a zero operative is refused as before, and
  offers that cannot be read stay 503 `unavailable`, because the terms are
  needed to buy. `item.kid` may be null end to end, including in the
  completion answer. Adoption resolves the KID from `metadata.json` and
  checks it against the chain binding; when it cannot, adoption stays
  `pending` (Download copy retries), and a proven mismatch writes no mint
  and is logged. Offer terms stay corroborated by two finalized sources.
  Supersedes R15 for an unreadable document and the binding check's gating
  role; the three legs remain readability and adoption evidence.
- **R47 Bounded retry on HTTP 429.** Every `evm_rpc` read, and a batch
  request, retries an HTTP 429 at most twice: after `Retry-After` seconds
  when present and at most 2, otherwise after 250 ms and then 750 ms. Other
  errors are not retried. Each retry is logged with the host only, never
  the full URL. A source that still answers 429 cools down for 60 s per
  origin: every corroborated read orders cooled sources last and keeps
  reading further configured sources, and still uses a cooled source when
  nothing else answers. A corroborated read stops once two observations
  agree on the same finalized block hash and the same tuple.
- **R48 Page copy for an unreachable market.** A `buy_offer` refused as
  unavailable — the object provider's `library_error` envelope carrying
  `Runtime custody purchase is unavailable` — shows "The market couldn’t be
  reached. Try again." and leaves Buy pressable. "The purchase did not
  finish." stays for a real failed purchase. Readability `unknown` puts no
  "Opens on ela.city" or other where-it-opens copy on the sheet, keeps Buy
  enabled, and a catalog row treats it as not learned yet.

- **R49 Explore opens on the market.** The market index answered a
  cold-cache catalog query in 12.7 s (warm: about 2 s) against a 6 s Runtime
  timeout, so the first Explore load always failed and showed this Home's
  own items until refresh. Runtime now allows 20 s in total with a 4 s
  connect, retries a transport failure once, and logs the full error chain.
  The page shows a loading state, retries once, then says "Couldn't reach
  the market." with Try again; it never presents local items as the
  market. Refresh only re-reads.
- **R50 Ownership and access proof keyed by the item.** With the KID
  unknown (R46), `buy_offer`'s already-owned check and the post-buy access
  evidence use `AuthorityGateway.hasAccess(address accessor, address
  ledger, uint256 tokenId)` (`0xcf56b4eb`), the same `_checkUserAccess` as
  `hasAccessByContentId`. An unknown KID cannot open a double payment, and
  completion rests on chain access again rather than the receipt alone.
  Once adoption proves the KID, the KID-keyed answer replaces it. Cost if
  wrong: none beyond one extra corroborated read.
- **R51 Owned labels follow what this Home holds.** "In your library" only
  when this Home holds a copy. A bought item without one reads
  "Purchased", with Download copy when adoption is pending, or "Opens on
  ela.city" when it is foreign. A Details view shows the item's properties
  from its `ListingObject` and never the account address.
- **Verified-listing pinning (defect found installed).** The purchase
  builder's verified-listing read compared sources without pinning them to
  a common finalized block. This had never shown while every source was a
  `base.org` endpoint with the same head; with independent providers it
  refused every Buy as "disagree". It now pins like the other market reads
  (R32). The defect predates this branch.

## Open questions

1. Read offers at head instead of the finalized block? The contract protects
   the buyer either way; head makes new and changed offers visible sooner.
2. Should adoption also cover items a principal holds access to without a
   market purchase, for example by transfer?
3. Where do ela.city and SDK mints place `kid` and `properties.publisher`?
   Verified on live items before implementation; the binding check depends on
   the KID.
