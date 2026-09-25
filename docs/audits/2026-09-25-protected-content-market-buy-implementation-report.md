# Protected-content market buy — implementation report

Date: 25 September 2026. Branch `feat/protected-content-listing-buy`,
stacked on PR #62 (`feat/protected-content-0.7.1-followup`, head
`43585914`). Design and every ruling:
[`2026-09-24-protected-content-market-buy-design.md`](2026-09-24-protected-content-market-buy-design.md).
Operator-facing flow: [`docs/PROTECTED_CONTENT.md`](../PROTECTED_CONTENT.md).
Open work: `TASKS.md`, "Buy from any live offer".

## What this delivers

A buyer can buy one copy of any protected item that has a live offer on
chain, whoever minted it — this Runtime, `base.ela.city`, or the SDK — found
by a pasted link or in Explore. The purchase runs on chain terms alone.
Reading (opening, playback) stays a separate workflow with its own dataset.

| Before | After |
|---|---|
| Buy needed a `listing.json` package the minting Runtime wrote | Buy needs only the item `(ledger, tokenId)` and a live offer |
| One seller per listing, terms frozen in the package | Every live seller for the item, terms read from chain at buy time |
| Listing and reading shared one dataset | Two datasets: the buy dataset (item + agreed offer) and the read dataset (shared `metadata.json`) |
| Items minted elsewhere could not be bought | Any item with an offer can be bought; readability is advisory |
| Content availability could block a purchase | Only chain terms gate a purchase (R46) |

## Architecture

- **Chain-provider** (`capsules/chain-provider`): item, KID-binding,
  offers, item-keyed access and verified-listing reads. Each read is
  corroborated by two independent sources pinned to a common finalized
  block (R32). A source that returns 429 gets two short retries; if it keeps
  returning 429 it cools down for 60 s and the read carries on with the
  other sources. A read stops asking once two sources agree (R47).
- **Runtime server** (`elastos-server`):
  - `POST /api/apps/marketplace/listing` builds a `ListingObject` from a
    token URI, a KID or an item (D15). Its `readability` is `verified`,
    `unverified`, `foreign` or `unknown`.
  - `buy_offer` buys on the agreed terms. It answers `terms_changed`,
    `attempt_in_progress`, `own_offer`, `already_owned` or `asset_mismatch`
    where they apply.
  - Each buyer has a market purchase record, created once under a lock and
    updated or retired only by the attempt that owns it (R26).
  - A buyer holds at most one open purchase per item, across both `buy` and
    `buy_offer` (R29). A declined or reverted attempt is retired (R41).
  - After a purchase, adoption turns an ElastOS-protected item into the
    records the existing open path reads.
- **Marketplace capsule**:
  - A strict parser for listing objects and buy answers.
  - An offer sheet listing every live seller.
  - Buying from a link or from Explore.
  - Continuing a recorded attempt.
  - Download copy for an item still waiting for adoption.
  - Explore opens on the market.
  - Owned-state labels by what this Home actually holds.
  - A Details view of an item's properties.

## Installed test, 25 September (Base mainnet)

An ERC-20 (USDC) offer, bought from Explore by a buyer Home on Base
mainnet. The item was minted on `base.ela.city` with Lit-Protocol CEK
custody.

| Step | Result |
|---|---|
| Listing and offers | read in 1.6–5.7 s per chain read |
| Item-keyed ownership check (R50) | not owned → continue |
| Verified terms, pinned across sources | agreed |
| Wallet: USDC `approve`, then `buyAccess` | approved by the person |
| Transaction | `0x4d8ba231db31d81239af3257832335b3d951578bd150eb4ba2a2aba4b971bc12`, mined |
| Access evidence | `has_access: true` at block 51786085 |
| Record | `complete`; KID proven from the shared document and the chain |
| Adoption | `foreign` (Lit custody: no ElastOS entry, so no local copy) — as designed |

Defects found by the run, and their fixes (all on this branch):

1. The ipfs-provider stopped an idle Kubo from a background thread, while
   the provider still marked it Ready. The fetch paths skipped the liveness
   check, so every IPFS fetch failed until restart. Fixed: every use goes
   through `ensure_kubo`.
2. The public Base RPCs (`mainnet.base.org` and
   `developer-access-mainnet.base.org` share one quota) answered HTTP 429
   under one Buy press. Fixed by R47, and the installed config now uses
   four independent providers: publicnode, blastapi, tenderly, drpc.
   `1rpc.io` answers HTTP 410 to these calls and was removed.
3. Metadata and KID reads blocked the purchase, against the owner's rule
   that content availability must not block a purchase. Fixed by R46.
   R50 then made the ownership and access checks item-keyed through
   `AuthorityGateway.hasAccess(address,address,uint256)` (`0xcf56b4eb`), so
   an unknown KID cannot allow a double payment.
4. The purchase's verified-listing read compared sources without pinning
   them to a common block, so independent providers at different finalized
   heights always "disagreed". This was present before this branch and was
   exposed by the new RPC set. Fixed: pinned like the other market reads.
5. The market index needs 12.7 s for a cold-cache catalog query (warm:
   about 2 s), against a 6 s Runtime timeout, so Explore always needed a
   refresh. Fixed: 20 s total, 4 s connect, one retry, the full error
   chain logged; the page shows a loading state and retries once (R49).
6. A foreign purchase was labelled "In your library". Fixed: labels follow
   what this Home holds, and a Details view shows the item's properties.

## Verification

At the final head:

| Suite | Result |
|---|---|
| Server broad filter set | 351 passed, 0 failed, 1 ignored |
| Server fmt / clippy `-D warnings` | clean |
| Chain-provider | 139 passed, fmt and clippy clean |
| Marketplace node tests | 109+ passed |
| Layout and behavior smoke | OK |
| `home-entropy-check`, `git diff --check` | pass |

Security and money behaviour went through four independent Opus reviews
(security/money, protocol/spec, capsule UI, tests/quality) and scoped
re-reviews of each fix round. The last one approved, with no Critical or
Important findings open.

## Still open

- **Installed journeys not yet run:**
  - terms change;
  - resale with two offers;
  - an ElastOS-protected (v1) item proving adoption, Download copy and
    playback;
  - a link-based buy.
- **Deferred follow-ups** (TASKS.md, R37):
  - an ipfs-provider `cat` byte cap;
  - typed provider errors for not-found and unavailable;
  - chain-read access state on the listing for KID-less items;
  - the purchase transaction on the Details view;
  - a keyed RPC for production;
  - fewer chain reads per Buy.
- **J5 acceptance** (two principals, playback, external crypto review #48)
  stays on the release plan as Later.
