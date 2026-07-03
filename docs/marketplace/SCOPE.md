# Marketplace — Corrected Scope (buy/trade venue for ALL dDRM assets)

> Supersedes the mint+playback framing in the earlier PLAN/layouts. Grounded in an audit of the runtime
> (players, creator, IPFS-pinning, dDRM boundary) + an adversarial verification pass. Citations are to the
> runtime worktree unless prefixed `PC2:` (the separate `~/Documents/Cursor/pc2.net` reference implementation).

## The one-line scope
**The Marketplace is the single place you go to discover, buy, trade, list, and withdraw dDRM assets.**
It **mints nothing, plays/renders nothing, holds no signer/token/CEK/IV/RPC.** It produces two things and stops:
(1) an on-chain **access right** (unsigned `buyAccess` → your wallet signs → broadcast), and (2) the bought
**encrypted file pinned into your local Library** — managed like any other file. The existing **player** opens it;
the existing **creator app** mints it.

## One marketplace, all asset types (the decided direction)
The Marketplace represents **all assets** — content now; **apps & games later** (dDRM will wrap executables too).
One storefront; **fulfillment branches by asset type**:

| Asset type | Discover / Buy / Trade / List | Fulfillment on buy |
|---|---|---|
| **Content** (watch/listen/read/view/explore) | unified | pin encrypted file → Library, open in **player** (`/api/viewers/open`) |
| **App / Game** (future) | unified | **install** via the existing app-store path (capability grant + consent) |

Rationale: discovery/buy/trade/list/withdraw/royalty + the dDRM access-right model are identical across types, so
the storefront unifies. The privileged "install = grant capabilities" step stays in the existing app-store install
machinery — invoked only as the app/game *fulfillment* — so capability hygiene (P3/P5) holds and there is one
canonical path (P10). **Consolidation:** evolve this content shell into the canonical Marketplace, make it
asset-type-aware, wire it into the launcher, and fold `capsules/marketplace` (today's app-store) in as the "Apps"
fulfillment — retiring the duplicate storefront. Implement the Content type now; scaffold Apps/Games in the IA.

## In scope
- **Discover / search / facets** over `content-market` listings (calldata→`ContentListingV1`, `contentId == bytes16 KID`, re-derivable index, re-verified live before a buy). Asset-type aware (Content live; Apps/Games scaffolded).
- **Buy** — assemble an **UNSIGNED** `buyAccess` tx, route to **wallet-provider** (human-in-loop), chain-provider broadcasts, await `hasAccessByContentId == true`. **Must use the unsigned→external-wallet path, never the `ELASTOS_DDRM_BUY_SIGN=wallet` managed-account autosign mode.**
- **On-buy pin-to-Library** — after the right is granted, **trigger** (never perform) a consumer-side acquire: pin the encrypted CID via the `content/*` plane (`content.rs ensure` → `local_pinned` receipt) and register it as a `LibraryObject` under the buyer's principal root. Surface pin progress. *(New build item; the design is proven in `PC2: ContentSeedingService.seedContent` + `pinned_cids` + `autoPinPurchases`, to be re-implemented in runtime Rust.)*
- **Trade / list / withdraw** — secondary resale of **owned** access: UNSIGNED `sellAccess` (`0x9a3fa9f5`, pre `setApprovalForAll 0xa22cb465`) + `withdrawListing` (`0x3e65bbba`), wallet-signed; manage your listings in the Vault.
- **Open handoff** — for owned assets, hand off to the existing open path (`POST /api/viewers/open {uri}` with `x-elastos-home-token`, or emit a Library open launch that routes through the home shell's `launchOwnedFromLibrary`); the runtime picks `elacity-player`/`ddrm-viewer` by mime/`required_interface`. The Marketplace opens the returned `play_url` and **renders nothing itself**.

## Out of scope (lives elsewhere in the runtime)
- **Minting / encrypt / KID-seal / IPFS-upload** → the `creator` capsule (+ `PC2: /api/media/encode`, `dashPackager`). Sale terms are baked into the mint, so **primary listing == minting** — not a marketplace step. *(No "Create" entry in the marketplace per direction — minting is reached from the creator app.)*
- **Playback / rendering / decrypt / CEK / IV / key release** → `elacity-player`, `ddrm-viewer`, `decrypt-provider`, `key-provider`. The marketplace embeds no `<video>`/MSE and no secure "stage."
- **Viewer selection** → the runtime decides (`is_media_mime` / `viewer.required_interface`); the marketplace just opens the returned viewer.
- **The drm/open sequence** (`content_status→content_fetch→rights_check→key_release→decrypt_session→render`) → planned by `drm-provider`, never called by the marketplace.
- **Raw IPFS pin/cat** (`elastos://ipfs/*`) → capsules use `elastos://content/*` (P4); the marketplace never calls `ipfs-provider` directly.
- **In-app "secure preview" egress** ("rendered, never downloaded / source never delivered / no download, ever") → removed. Asset detail shows a **poster/cover only** (optionally a pre-rendered public teaser produced at encode time that needs no decrypt-provider). **No new FREE-scope secure-preview trust boundary.**

## Verified integration seams (real, in this repo)
- **Open handoff:** `POST /api/viewers/open` → `{schema:"elastos.viewer.open/v1", viewer, session, title, play_url, rights_binding}` (`viewer_open.rs:213,1506`); reference consumer `capsules/home/browser/shell-windows.js:978-1061` (open → on-403 buy → retry → iframe window). Runtime app-launch is **`runtime-target` + `postMessage`** (`shell.js:99,504,527`), **not** `puter.ui.launchApp` (that was a mis-spec from the PC2 layer).
- **On-buy pin:** `content.rs ensure()` pins + writes `status="local_pinned"` (`content.rs:3591+`); `ipfs-provider` `pin`/`cat_to_path`/`download_directory` (`ipfs-provider/src/main.rs:268-275`). **Missing:** a consumer `Acquire`/import-by-remote-CID op on `object-provider` (`library.rs` `ObjectProviderRequest` enum has only producer + local-file ops) — the one new runtime seam.
- **Buy:** `buy_authority::buy_access` (`buy_authority.rs`) — orchestration only, no keys/RPC; returns unsigned tx absent an opt-in signer.
- **dDRM boundary:** Marketplace produces (on-chain right) + (pinned encrypted CID); consumes nothing of rights/key/decrypt. Handoff id = `bytes16 KID == on-chain contentId == object.key_envelope.kid`.

## Hard gates / corrections folded from the verification pass
- **Phase-1 buy-invariant — IMPLEMENTED** (compile + unit-tested; live-chain test pending). `buy_authority.rs` now binds terms to the re-read on-chain `listings(operative, tokenId, seller)` with abort-on-drift, computes `value = price×quantity`, approves the Operative `paymentProcessor` (ERC-20), and resolves the REAL ledger tokenId via the `chain-provider` KID→tokenId resolver (`AssetCreated` + mint calldata, **fail-closed on ambiguity**) — never `word_from_id(content_id)` on the live path. Remaining: the live-chain integration test + sourcing seller/price live from `sellersOf` (today **fail-closed if unset**, not buyer-defaulted). The real buy button still ships only after the live-chain pass (P11 fail-closed throughout).
- **Buy path is unsigned→external-wallet only** (not the managed-account autosign mode).
- **Two-capsule reconciliation** — `capsules/marketplace` (launched app-store) vs `capsules/marketplace-content` (this content shell, unwired). Consolidate to ONE canonical Marketplace per the direction above; do not maintain two storefronts (P10).
- **PC2 is a separate-repo reference** — its download-first/pin flow is real and shipped, but must be **re-implemented in runtime Rust**, not "ported" file-for-file.

## Open items
- The runtime app-launch path for cross-app handoff (open is solved via `/api/viewers/open`; a future "install app/game" fulfillment reuses the app-store install path).
- The `object-provider Acquire` op + the buy→pin wiring (new Rust; Cursor).
- Confirm the Operative **primary-price getter** + the **KID→ledger tokenId** resolver (CONTRACTS.md §7).
- Apps/Games asset-type fulfillment (install) — scaffolded now, wired when dDRM wraps executables.
