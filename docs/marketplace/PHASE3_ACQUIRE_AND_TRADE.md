# Phase 3 — Acquire (buy→pin) · Resale/Withdraw · Open-handoff (turnkey for Cursor)

> The post-buy + trade backend, grounded in the audited runtime seams ([SCOPE.md](SCOPE.md)) + verified contracts ([CONTRACTS.md](CONTRACTS.md)). The marketplace **triggers**; the runtime providers do the work; signing is wallet-only (unsigned→external). Build order: **Phase 1 (buy-invariant) gates everything**; then 3A Acquire ∥ 3B Resale/Withdraw ∥ 3C Open-handoff.

## Confirmed inputs (empirical, this pass; verified against the runtime + PC2)
- **Listing enrichment** comes from `metadata.json` (fetched by the index over the IPFS gateway `ipfs.ela.city`): `name, description, image, category` · `asset{cid, mimeType, size, encrypted, algorithm, dataToEncryptHash}` · `media{uri:"ipfs://<cid>"}` · `properties{chainId 8453, ledger, authority 0x09dBe7, publisher, categories, distribution}`.
  - ⚠️ **`metadata.pricing{}` is LEGACY (v1.0)** — PC2's own CHANGELOG records that v1.1+ assets no longer embed pricing inline, and resolving op_type/price from `metadata.pricing` was a *bug*. **Authoritative price/supply/op_type come from the on-chain Operative** (`AssetCreated` + `sellersOf`/`listings` + the sell-terms calldata), per CONTRACTS.md — the index must NOT trust inline `pricing{}` for money.
- **The CID pinned on buy = `metadata.asset.cid`** (the encrypted media object; `media.uri` = `ipfs://<cid>`). Pinning it grants **NO** decryption — the CEK is recovered separately at open by the 2-of-3 quorum behind the rights gate (PRINCIPLES.md §15). *(P-tags here map to PRINCIPLES.md headings: P4=§4 carrier plane, P10=§10 one canonical path, P15=§15 provider-mediated decryption, P16=§16 UI≠authority.)*
- **Price is on-chain in the sell-terms calldata** `quantity(32B) | pricePerToken(32B) | payToken(32B)`. The **live `listings()` decode** is PC2 `ContentIndexerService.ts:466-473` (lowest across listings); the runtime **`content-market`** decodes the same 96-byte `uint256|uint256|address` shape from the **mint-time `sellRawData`** (`copies|price_wei|payToken`, `content-market/src/main.rs:608-610`) — same layout, different source/semantics. Pay-token = canonical Base **USDC, 6 decimals**.

---

## 3A · Buy → pin-to-Library (the `Acquire` seam — the one genuinely NEW runtime op)
**Today:** `buy_authority::buy_access` records the access token only; nothing is pinned. `open_quorum_media` re-fetches the asset into an ephemeral `PlaintextTempDir` each open. The `library.rs` `ObjectProviderRequest` enum has only producer + local-file ops — **no consumer import-by-remote-CID**.

**Build:**
1. **`ObjectProviderRequest::Acquire { principal_id, content_cid, metadata }`** (new variant, `library.rs` enum ~`:151-323`). Creates a `LibraryObject` under the buyer's principal root (`localhost://Users/<principal>/…`) with `content_cid = asset.cid`, `availability = "local_pinned"`, `mime` + `name` from metadata, `published=false`. Idempotent (no-op if already present). Returns the Library `uri`.
2. **Pin** before/within Acquire: call the **`content/*` plane** `ensure` (`content.rs:3591+` → `ipfs-provider pin`, writes `status="local_pinned"`) — NOT raw `elastos://ipfs/*` (P4). Pull the encrypted bytes into `~/.local/share/elastos/ipfs-repo`.
3. **Gateway route `POST /api/market/acquire { content_id }`** — resolves `asset.cid` from the (re-verified) listing/metadata, runs `ensure`, then `object-provider Acquire`. Returns `{ pin_status, bytes_downloaded, library_uri }`. The shell's `api.js` already defines `API.acquire` (mock); wire it to this live route and poll for progress.
4. **Open reads local first:** the open path should serve the pinned CID from the local blockstore rather than re-fetch into a throwaway dir each play (`viewer_open.rs:174-206,1329-1372`).

**Acceptance:** after a confirmed buy, the encrypted asset is a `LibraryObject` in the buyer's file tree, managed like any other file; `pin_status` reaches `complete`; opening it works offline from the local repo.
**Invariants:** marketplace TRIGGERS only (no pin/write/keys itself); encrypted-only; idempotent; pin-status surfaced (no silent stall).

## 3B · Resale + withdraw assemblers (secondary market — the legitimate marketplace listing)
**Today:** there is **no Rust list/cancel/offer assembler anywhere in the runtime** (the `marketplace` capsule has only a stub `wasm/main.rs` + browser UI; a grep for `sellAccess`/`withdrawListing`/`sellToken`/the selectors across all `*.rs` returns zero hits). The UX is built (`openResale`, Vault "Listed", `openWithdraw`).

**Build** (pure, selector-pinned, UNSIGNED → wallet; mirror `buy_authority` discipline — no keys/RPC):
- **PRE: `setApprovalForAll(operator=AuthorityGateway, true)` `0xa22cb465`** — approve the gateway to move the buyer's ERC-1155 access token (once per owner). **NB:** this is an **Operative/ledger (ERC-1155) method, NOT a gateway method** — keccak-correct but deliberately absent from the AuthorityGateway bytecode (its sibling `isApprovedForAll 0xe985e9c5` *is* present on the gateway); **confirm against a real deployed Operative before relying.**
- **`sellAccess(ledger, tokenId, quantity, pricePerToken, payToken)` `0x9a3fa9f5`** — list owned access for resale. `payToken = USDC 0x833589fC` (6dp); `pricePerToken` in minor units. Bind the **reseller as seller**; require a live `hasAccessByContentId(subject, KID)==true` proof (anti-spoof) before assembling.
- **`withdrawListing(ledger/operative, tokenId, quantity)` `0x3e65bbba`** — cancel; access right unaffected.
- **TradeGateway `0xd02451…`** secondary surface (`sellToken/buyToken/createOffer`) if offers are in scope; else AuthorityGateway list/withdraw suffices.
- Resolve the **real ledger `tokenId`** (NOT `word_from_id(content_id)`) from `DigitalAssetRegistered`/`AssetCreated` keyed by `bytes16 KID` — shared with Phase 1.

**Acceptance:** owner lists → a secondary listing with floor appears in the index; another wallet buys it; royalty splits (incl. `resellerCut`) enforced on-chain; withdraw removes the listing, access intact.

## 3C · Open-handoff (marketplace renders nothing)
**Use the existing path verbatim:**
- Owned CTA → `POST /api/viewers/open { uri: <library_uri> }` with header `x-elastos-home-token` (optionally `grant_handle` + `delegation_sig_hex` from `POST /api/viewers/prepare-grant` → wallet `personal_sign` for chain-mode dKMS). Returns `{ viewer, session, title, play_url, rights_binding }`; open `play_url` (`/apps/{viewer}/?session=&home_token=`) as an iframe targeting the returned `viewer`. Cleanest: emit a **Library open launch** `{uri}` and let the home shell's `launchOwnedFromLibrary` run open(+buy-retry) (`shell-windows.js:978-1061`).
- The marketplace must NOT build a session, fetch `/media|/object/{session}`, call decrypt-provider, or embed `<video>`/MSE. The runtime picks `elacity-player`/`ddrm-viewer` by mime/`required_interface`.

**Acceptance:** clicking "Open in your library" on an owned asset opens it in the existing player from the local Library; the marketplace contains no render code.

---

## Order & gates
1. **Phase 1 (buy-invariant)** — re-read `(seller,tokenId,price,payToken,supply)` live + abort-on-drift; real KID→tokenId resolver; **unsigned→external-wallet only**. No buy button before this. *(gates 3A/3B)*
   - ⚠️ **doc/code tension to reconcile (P12):** `buy_authority.rs` ships a managed-account autosign mode (`ELASTOS_DDRM_BUY_SIGN=wallet`, commented "RECOMMENDED") that signs on-box without a human. The user-facing buy button **must reject** that mode and use the unsigned→external path (`buy_authority.rs` returns the unsigned tx as HTTP 409 absent the opt-in). Either align the spec to acknowledge the autosign option it rejects, or update the in-code "RECOMMENDED" comment so docs/code agree.
2. **3A Acquire** ∥ **3B Resale/Withdraw** ∥ **3C Open-handoff** — independent once Phase 1 + the tokenId resolver land.
3. **Wire shell → live** (`api.js` mock → real `/api/market/*`; mock under `?mock=1`).
4. **Canonical surface** — one Marketplace for all assets; fold the app-store in as "Apps" fulfillment; launcher wiring (P10).

All money-path/chain/Library Rust is **Cursor-implemented + chain-tested + signed off**; this spec makes it mechanical.
