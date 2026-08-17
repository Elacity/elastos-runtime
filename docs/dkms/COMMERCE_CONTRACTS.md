# Marketplace Contracts — Turnkey Backend Reference (Base mainnet)

**Status:** decision-resolved, but **read the Verification status below before any mainnet write** — some items are confirmed in this repo and some are single-source (grounded in elacity-web/PC2, not yet checked against deployed bytecode). Cross-referenced across three sources:

1. `elacity-web` `release/base-network` — ABIs in `src/lib/drm/contracts/*.json`, address map `src/lib/web3/Ecosystem.tsx`.
2. PC2 node (`pc2.net/pc2-node`) — `data/installed-apps/elacity-market/wallet.js` (canonical client ABI strings + selectors), `src/services/ContentIndexerService.ts` (event topics + read selectors), `config/default.json` (`content_indexer.contracts.v3`).
3. This runtime — `capsules/chain-provider/src/{abi.rs,main.rs,config.rs}`, `capsules/content-market/src/main.rs`, `elastos/crates/elastos-server/src/api/buy_authority.rs`.

All 4-byte selectors in this doc were **independently recomputed with keccak-256** and cross-checked against the pinned constants in the three sources. Where a selector is newly computed (no existing pin), it is flagged `[COMPUTED]`.

## ⚠️ Verification status (honest confidence — read before any mainnet write)
- **Selector math: certain; ABI match: not.** All 20 selectors were keccak-recomputed — but **keccak-correct ≠ ABI-correct**: a selector is only right if the *deployed* function signature matches the one assumed here.
- **Confirmed in THIS repo (rely on these):** `AuthorityGateway 0x09dBe7…` and `USDC 0x833589fC…` are pinned identically in `buy_authority.rs:58` + `chain-provider/main.rs:102`; and the buy-path selectors `buyAccess` (`0xf7580ad9` native / `0x0ede2294` ERC-20) + `hasAccessByContentId 0x54d42821` match the runtime's own pins.
- **✅ EMPIRICALLY CONFIRMED on the deployed chain (`verify-selectors.mjs`, verified 2026-06-22 on Base mainnet):** the AuthorityGateway `0x09dBe7…` is an **EIP-1967 proxy → implementation `0x305e37267b7a9eafbfed6b380d8cad9117a265d1`** (12,137 bytes), and **all eight core selectors are present in the deployed bytecode**: `buyAccess` (native `0xf7580ad9` + ERC-20 `0x0ede2294`), `hasAccessByContentId 0x54d42821`, **`sellAccess 0x9a3fa9f5`**, **`withdrawListing 0x3e65bbba`**, **`paymentProcessor() 0xf1c6bdf8`** (the ERC-20 approve target), `sellersOf 0x997eab2d`, `listings 0x6bd3a64b`. So the entire **buy / list / cancel / has-access / payment** surface is keccak-correct AND on-chain-confirmed — and **list+cancel live on the SAME gateway** (there is no separate contract for them). Re-run `verify-selectors.mjs` after any proxy upgrade.
- **✅ Secondary market RESOLVED (live):** a distinct **TradeGateway `0xd02451BCE627EF476B8ee52Cf131C426f67dbcB2`** is deployed on Base (EIP-1967 proxy → impl `0xe60433e553a35091571471a93a49d86d3223a59f`, 10.5 KB) with all four secondary selectors present: `sellToken 0xad1ee6be`, `buyToken 0x7d17ff3d`, `createOffer 0xd898aaf2`, `withdrawListing 0x3e65bbba`. So **primary access market = AuthorityGateway `0x09dBe7…`; secondary royalty/offers market = TradeGateway `0xd02451…`** — both real, both live-confirmed.
  - ⚠️ **Address reconciliation:** the elacity-web v3 audit reported a base-network `TRADE_GATEWAY = 0xDe239B63949948FaC2A21aaa39bE0cd4775b1763`, but **that address has NO Base bytecode** (`eth_getCode` empty) — a wrong-network read (a non-Base Ecosystem.tsx row). The live, selector-bearing TradeGateway on Base is **`0xd02451…`** (confirmed twice via bytecode). Do not "correct" it to `0xDe239B`. The TradeGateway (royalty-share, tokenId=2) is a SEPARATE system from the access-token market — the runtime's core resale path is `AuthorityGateway.sellAccess`.
- **⚠️ Buy ERC-20 approve spender (grounded in elacity-web v3):** the `approve(spender, MaxInt256)` leg for an ERC-20 (USDC) buy targets the asset's **Operative `paymentProcessor()`** (read live from the operative) — **NOT the AuthorityGateway.** Allowance is pre-checked against `paymentProcessor`. The native (AddressZero) path needs no approve; `value = pricePerToken × quantity`. (Sell side: ERC-1155 `setApprovalForAll(operator = AuthorityGateway, true)` is sent to the **Operative**; `sellAccess` arg0 = **ledger**, `withdrawListing` arg0 = **operative** — an intentional asymmetry; the gateway maps ledger↔operative.)
- **✅ Confirmed Base-8453 address set** (elacity-web `release/base-network` 8453 block × live bytecode): AuthorityGateway `0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D` (→`0x305e3726…`) · TradeGateway `0xd02451BCE627EF476B8ee52Cf131C426f67dbcB2` (→`0xe60433e5…`) · CoreStorage `0x0C1EeA2A3361B80AC0e42179335dB536A951760b` · ChannelCore `0xE1365ed47353De2F8A6a69E271e36650A9EE368F`. **Runtime↔elacity reconciliation:** both drive the **identical** AuthorityGateway on Base — the `0x47275C…`/`0xE89B4d…` gateways are *other chains* (no Base bytecode), not a conflict.
- **✅ Pay-token CONFIRMED = canonical Base USDC `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`** (deployed; used in PC2's own code `pc2-node/dist/api/index.js:909`; matches the runtime's pin). **There is NO WELA on Base** — `WELA 0x517e…` and `USDC 0x175F…`/`0xA06b…` are in elacity-web's `currencies` map under **ESC chains 20/21**, NOT Base 8453 (a corrected earlier cross-block misread). On Base: **gas = ETH, listings/payments = USDC `0x833589fC…`** (PC2 SSOT also lists USDT `0xfde4C96c…`, DAI `0x50c57259…`, WETH `0x42000000…0006` as accepted pay-tokens).

### ✅ PC2-SSOT reconciliation (2026-06-22 — the user's consolidated `pc2.net` reference)
- **Addresses all confirmed** against the runtime pins: CentralStorage `0x0C1EeA2A…`, AuthorityGateway `0x09dBe796…`, ChannelFactory `0xE1365ed4…`, **RoyaltyTradeGateway `0xd02451BC…`** (this VINDICATES keeping `0xd02451` over the elacity-web-audit agent's `0xDe239B…`, which has no Base bytecode = a wrong-network read), EventHub `0x5a694A6d…`. Additional (not previously banked): **AssetFactory `0x4c80A6209F16437f0dc4a98E3D43f08aeBF57765`**, **SubscriptionManager `0xb00456b5…`**, and the extended channel/operative factories (PublicChannel `0xfcDffDd1…`, PrivateChannel `0x6d0369f5…`, MultiChannel `0x2E8B108a…`, BuyableOperative `0xFbf39a09…`, BuyableSellableOperative `0xd4FE224a…`).
- **⛔ V2-deprecated — DO NOT USE:** CoreStorage `0xc8F50Bf1…`, AuthorityGateway-V2 `0x8fe6bf98…`, ChannelCore `0x6a3f7780…`, TradeGateway-V2 `0x9eC53758…`. (These still appear in stale docs/compiled `.js`.)
- **⚠️ Mint event correction (empirically verified on Base):** the live mint event is **`AssetCreated`** (emits from EventHub; KID is in the mint `opRawData` calldata, NOT the event). **`DigitalAssetRegistered` does NOT emit on Base** (0 logs anywhere in an active window) — so the KID→ledger-tokenId resolver binds the KID via the mint **calldata** (`mint_input_binds_content_id`), keyed off `AssetCreated`, NOT a `DigitalAssetRegistered` event. (`content-market` still *decodes* DigitalAssetRegistered for the calldata-identity case, but nothing emits it on-chain.)
- **Lit/Particle are PC2 infra the runtime does NOT use:** the runtime replaces Lit (chipotle PKP/TEE) with its own `decrypt-provider` + 2-of-3 dKMS quorum, and Particle smart-accounts with `wallet-provider`. Do **not** port §5/§9 Lit/Particle specifics; the access gate `hasAccessByContentId(holder, bytes16)` and the contract surface are shared, the key/decrypt infra is not.
- **Per-asset authority:** prefer `metadata.properties.authority` over the hardcoded gateway (the dApp itself is inconsistent; the runtime's `0x09dBe7` matches current assets).
- **✅ Canonical v3 indexer config** (PC2 `pc2-node/config/default.json` → `content_indexer.contracts.v3`, chain 8453 — the source of truth for Phase 2): `authority_gateway 0x09dBe796…` · `channel_factory 0xE1365ed4…` · `central_storage 0x0C1EeA2A…` · **`event_hub 0x5a694A6d988354dca491fe0F6db7a6ef46b656c2`** (the index's event source — **live**, resolves the earlier "missing EventHub" gap) · `from_block 43892000` (backfill genesis) · scan **5 min / 10 000 blocks** over the Base RPC subset · IPFS gateways `ipfs.ela.city`, `dweb.link`, `cloudflare`. Phase-2 reads event logs from **EventHub**, not the channel/asset contracts directly.
- **✅ Phase-2 event topics — COMPLETE (V3, source-confirmed in PC2 `sdk/config.ts:174`+`ContentIndexerService.ts`; `AssetCreated` observed live on EventHub):** the index `getLogs` the EventHub for —
  - **`AssetCreated(address indexed _to, address indexed _channel, uint256 _tokenId, string _tokenUri, uint16 _opType, address indexed opContract)`** — topic0 `0xc0a995e4052be044599af577ab2f3382d67bd34df95a76226e7c464e9d4dba46`. Carries the **Operative address (`opContract`)** → the per-asset contract for `sellersOf`/`listings`/`paymentProcessor`/supply reads.
  - **`DigitalAssetRegistered(address indexed channel, uint256 indexed tokenId, address creator, string tokenURI, uint16 opType, bytes16 contentId)`** — topic0 `0x1b24f7763272894608506beba5887c374d345cd231bf52bd03f40bc2d0508d7b`. Carries the **`contentId` (== bytes16 KID)** — the trust anchor `content-market` validates metadata against.
  - **`ChannelCreated(uint8 indexed channelType, uint8 indexed scope, address indexed creator, address channel, address factoryAddr)`** — topic0 `0x4ae6ef95ddade103ca67593cd4cf68dda177aa1054ad4eeb4963d2c3df44702e`.
  - plus ERC-1155 `TransferSingle 0xc3d58168…` + `ContractCreated 0x2d49c679…`. IPFS: upload `https://base.ela.city/api/v2/ipfs/upload`, gateway `https://ipfs.ela.city/ipfs`.
  - **✅ Empirically confirmed (`index-proto.mjs`, live Base): EventHub emits `AssetCreated` ONLY** — a working scan decoded **50 real listings** (channel `0x6756e140…`, real Operatives, real IPFS `metadata.json` URIs, all `buy_and_resell`). **`DigitalAssetRegistered` + `ChannelCreated` are emitted by the channel/factory contracts, NOT EventHub** — so the index resolves `contentId`/KID from the asset's **`metadata.json`** (via `tokenURI`; `content-market` validates `metadata.kid`) and reads **price/supply from the Operative** (`sellersOf`/`listings`). Phase-2 discovery is now *proven against the real chain*, not just specced — see the runnable `index-proto.mjs`.
- **Proxy-upgrade watch:** both gateways are upgradeable — pin impls `0x305e…` (Authority) + `0xe60433…` (Trade); re-run `verify-selectors.mjs` on any upgrade.
- **Remaining judgment call:** the `ChannelBridge (bytes32,address)` hasAccess mismatch (different selector + registry `0x96826e93…`, absent from the runtime) — still unconfirmed; likely a stale elacity path.
- **Resolved by judgment, not proof:** `ChannelBridge.ts`'s `hasAccessByContentId(bytes32,address)` (= `0x594a4a6b`, a *different* selector on a *different* registry `0x96826e93…` absent from this runtime) was resolved in favor of the runtime's `(address,bytes16)` form on `0x09dBe7…`. Likely a stale elacity path — but confirm it isn't a genuinely different contract before relying.
- **Net:** the buy path's *core* (gateway + `buyAccess` + `hasAccess`) is verified in-tree; the **approve-target + secondary-market selectors and the non-gateway addresses are to-verify**. Treat this as a turnkey *starting point* whose unpinned items get a one-time bytecode check before value moves.

### ✅ LIVE PASS (2026-06-23 — read-only Base mainnet verification + one money-path fix)
Verified against real deployed state (public RPC, no wallet); decoded against real txs/logs:
- **`listings()` return word order CONFIRMED `(qty, pricePerToken, payToken)`** — decoded 3 real `ItemListed` events + cross-checked the live `listings(op,1,seller)` return at the event block; the decisive case (qty=10000, price=20000) is unambiguous. `buy_authority::decode_listing_return` was already correct; a stale doc-comment (said `(price,qty,…)`) was fixed.
- **🐞 FIXED money-path bug: `listings`/`sellersOf` must key at ACCESS_TOKEN id=1, not the content tokenId.** Confirmed live: `listings(op, 1, seller)` is populated (qty=9999, price=10000, USDC) while `listings(op, contentTokenId, seller)` returns an EMPTY slot (0,0,0x0). The runtime's `read_listing_terms` + `sellers_of_live` (and so the buy abort-on-drift re-read AND `/api/market/get`) were keying at the **content** tokenId → every live buy would abort (empty re-read ⇒ drift/sold-out) and `/get` would show no price. Fixed to read at id=1 while `buyAccess` keeps the content tokenId; regression test added (`listings_read_is_keyed_at_access_token_id_one_not_content_tokenid`).
- **`buyAccess` arg shape CONFIRMED** from a real ERC-20 buy (tx `0x64b70816…dcd56c`): selector `0x0ede2294`, `value=0`, `payToken=USDC`, `ledger`=per-channel ledger (NOT the gateway), `tokenId`=the big content tokenId (NOT 1, NOT `word_from_id`), `qty=1`, `price=10000`. Matches the runtime's assembled order.
- **KID→tokenId resolver CONFIRMED** against 3 real assets: a direct mint (`0x47cbeeb4`) resolves via the precise `decode_mint_content_id` (== `metadata.kid`); two **relayed** mints (`0xcef6d209`) resolve via the substring binder — proving the relayer-safe fallback is genuinely required on Base. Each KID uniquely bound its real ledger tokenId.
- **Event topic0 CONFIRMED** (`ItemListed`/`ItemSold` live; `ItemUnlisted` keccak-correct, sample-pending) — see §1.1 EVENTS.
- **Still requires a funded wallet (handed to the operator):** the unsigned→wallet→broadcast money path (buy grants `hasAccessByContentId`; wrong-token/drift aborts pre-broadcast), `/api/market/get` enrichment on a real asset, and `/api/market/acquire` (buy→pin→Library→open).

> **Single most important framing.** The "marketplace" is **not** one contract. It is:
> - **AuthorityGateway** — the **primary access-token market** (buy/sell the right to consume an asset; ERC-1155 sub-token id `ACCESS_TOKEN = 1`) **and** the EIP-712 license `verifyingContract` and the on-chain access oracle.
> - **TradeGateway** — the **secondary / royalty-share market** (buy/sell/offer the resale + royalty token; ERC-1155 sub-token id `ROYALTY_SHARE = 2`).
> - **Operative** — the **per-asset ERC-1155** contract that actually holds balances, roles, the payment processor, and `OP_TYPE`. There is one Operative per asset.
> - **CoreStorage** — registry / fee config.
> - The **legacy ESC NFT Marketplace + Auction** (`MARKETPLACE_ADDRESS` / `AUCTION_ADDRESS`) is a **separate, older** ERC-721/1155 fixed-price+auction system wired only for ESC chains 20/21. **It does NOT exist on Base.** Do not bind to it. Section 1.6 documents it only so nobody confuses the two.

---

## 0. TL;DR for Cursor — what to build

| Verb | Contract + method | Approval needed | Live reads first |
|------|-------------------|-----------------|------------------|
| **buy** (primary access) | `AuthorityGateway.buyAccess(...)` — native `0xf7580ad9` **or** ERC20 `0x0ede2294` | ERC20 only: `approve(operative.paymentProcessor(), price)` | `paymentProcessor()`, `sellersOf()`, `listings()`, real `tokenId` |
| **list** (primary access) | `AuthorityGateway.sellAccess(...)` `0x9a3fa9f5` | `operative.setApprovalForAll(AuthorityGateway, true)` | `isApprovedForAll()` |
| **cancel** (primary) | `AuthorityGateway.withdrawListing(...)` `0x3e65bbba` | none | `listings()` to know your qty |
| **buy** (secondary/royalty) | `TradeGateway.buyToken(...)` `0x7d17ff3d` | ERC20: `approve(TradeGateway, price)` | `TradeGateway.sellersOf/listings` |
| **list** (secondary/royalty) | `TradeGateway.sellToken(...)` `0xad1ee6be` | `operative.setApprovalForAll(TradeGateway, true)` | `isApprovedForAll()` |
| **cancel** (secondary) | `TradeGateway.withdrawListing(...)` `0x3e65bbba` | none | `listings()` |
| **offer** | `TradeGateway.createOffer(...)` `0xd898aaf2` / `0xa86d2604` | ERC20: `approve(TradeGateway, price)` | — |
| **discover** | `eth_getLogs` for `AssetCreated` + `DigitalAssetRegistered` + `ChannelCreated` | — | + `sellersOf/listings` for prices |
| **open / access gate** | `AuthorityGateway.hasAccessByContentId(holder, bytes16 kid)` `0x54d42821` (bool) + EIP-712 license cert | — | on-chain eth_call |

**Runtime seams to change:**
- `buy_authority.rs` — replace env-pinned `seller/ledger/tokenId/price/payToken` and the `word_from_id = SHA-256(content_id)` tokenId with **live reads** of `sellersOf`/`listings` and the **real ledger tokenId**, and **implement the ERC20 `approve` leg** (currently only flagged `requires_erc20_approve: true`, never assembled).
- `chain-provider` — add typed read methods `sellers_of`, `listings`, `payment_processor`, `allowance`, and `tokenURI`; add a periodic `eth_getLogs` scan for listing lifecycle events (today it only scans channel/asset-creation events).
- `content-market` — already decodes `AssetCreated` / `DigitalAssetRegistered`; extend its `ContentListingV1` to carry the **live price/seller** fields the indexer populates from `listings()`.
- **Resale assembler — BUILT (gateway-side):** the pure, selector-pinned `sellAccess` / `withdrawListing` / `setApprovalForAll` calldata assembler is in `elastos/crates/elastos-server/src/api/trade_authority.rs` (wired to `/api/market/order/{sell,withdraw,approve}`, Home-token-gated, address-validated, unsigned→wallet) — **not** the `capsules/marketplace/src` wasm shell (which is UI only). Still unbuilt: `sellToken` / `createOffer` / `cancelOffer` (the TradeGateway royalty-share path).

---

## 1. The contract interface

### 1.1 AuthorityGateway — primary access-token market + access oracle + license verifyingContract
ABI: `elacity-web/src/lib/drm/contracts/AuthorityGateway.json`. AccessControl-based, upgradeable proxy (`initialize()`).

**BUY (two overloads, selected by `payToken == AddressZero`):**

```
// NATIVE (ETH). value = pricePerToken * quantity
buyAccess(address seller, address ledger, uint256 tokenId, uint256 _quantity, uint256 _pricePerToken) payable
  selector 0xf7580ad9   (sig "buyAccess(address,address,uint256,uint256,uint256)")

// ERC20 (USDC default on Base). value = 0. REQUIRES prior approve(paymentProcessor, price).
buyAccess(address seller, address ledger, uint256 tokenId, uint256 _quantity, uint256 _pricePerToken, address _payToken)
  selector 0x0ede2294   (sig "buyAccess(address,address,uint256,uint256,uint256,address)")
```
> The web app and PC2 both **select the overload by the explicit full signature string**, not by guessing. A runtime that hardcodes one selector breaks the other payment path. `value` is set **only** on the native overload.

**LIST (sell access for resale):**
```
sellAccess(address ledger, uint256 tokenId, uint256 quantity, uint256 pricePerToken, address payToken)
  selector 0x9a3fa9f5   [COMPUTED from the verified ABI string in wallet.js / AuthorityGateway.json]
sellAccessOnBehalf(address seller, address ledger, uint256 tokenId, uint256 quantity, uint256 pricePerToken, address payToken)
```

**CANCEL (primary listing):**
```
withdrawListing(address operative, uint256 tokenId, uint256 quantity)
  selector 0x3e65bbba   [COMPUTED — same signature used by both gateways]
```

**READS:**
```
hasAccess(address accessor, address ledger, uint256 tokenId) -> bool          0xcf56b4eb [COMPUTED]
hasAccessByContentId(address accessor, bytes16 contentId)    -> bool          0x54d42821 (pinned, real Base ABI)
operative(address ledger, uint256 tokenId)                   -> address
listings(address op, uint256 tokenId, address seller) -> (uint256 qty, uint256 pricePerToken, address payToken)
                                                                              0x6bd3a64b (pinned)
sellersOf(address op, uint256 tokenId)                        -> address[]    0x997eab2d (pinned)
cstore()                                                      -> address
protocolVersion()                                             -> (used in EIP-712 license domain)
```

**EVENTS:**
```
ItemListed(address indexed seller, address indexed op, uint256 indexed tkId, uint256 quantity, uint256 pricePerToken, address payToken)
ItemSold(address seller, address indexed buyer, address indexed op, uint256 indexed tkId, address payToken, uint256 unitPrice, uint256 price)
ItemUnlisted(address indexed seller, address indexed op, uint256 indexed tkId, uint256 quantity)
PaymentLog(from, to, amount, paymentToken)
```
> **✅ topic0 CONFIRMED (live pass 2026-06-23 — keccak of the canonical v3 ABI signatures, validated by reproducing the pinned `AssetCreated` topic0, then matched against deployed AuthorityGateway `0x09dBe7…` logs):**
> - `ItemListed(address,address,uint256,uint256,uint256,address)` → **`0x90aecdd7f5269ac7f11bea516b4768d0391e0a54aabc19aea64c7758104f66d2`** — CONFIRMED on-chain (22 logs in an 800k-block window; sample tx `0x845f1b4a…abe1a5`, 4 topics + 96B data = the 3 indexed + `quantity,pricePerToken,payToken`).
> - `ItemSold(address,address,address,uint256,address,uint256,uint256)` → **`0x60cd9eee664e26e142eb54813d426c273cd85605b8bfb72f707e4f2927b6a955`** — CONFIRMED on-chain (tx `0x64b70816…dcd56c`, 4 topics + 128B data = `seller,payToken,unitPrice,price`).
> - `ItemUnlisted(address,address,uint256,uint256)` → **`0xdb6bedce61ad043a5e9d9ac95f248702233e64e5818e58734aa38e7fd86db415`** — keccak-correct from the same canonical ABI; not emitted in the scanned window (withdrawals are rare), so on-chain-sample-pending but topic0 is sound.
> The events index `tkId == ACCESS_TOKEN id 1` (NOT the content tokenId) — same keying as `listings`/`sellersOf` (§1.3).

**EIP-712 license domain (the access/decrypt gate):** `name: "AuthorityGateway"`, `version: protocolVersion()`, `chainId: 8453`, `verifyingContract: <AuthorityGateway address>`. Message = `LicenseRequest { entitlement, entity: { contentId(bytes16), ledger, tokenId } }`, signed via `eth_signTypedData_v4`. See `elacity-web/src/lib/drm/license/request.ts` + `usePlayerCertificate.tsx`.

### 1.2 TradeGateway — secondary / royalty-share market + offers
ABI: `TradeGateway.json`. Same `listings`/`sellersOf`/`withdrawListing` surface as AuthorityGateway, **plus**:
```
buyToken(address seller, address _contract, uint256 tokenId, uint256 _quantity) payable    0x7d17ff3d [COMPUTED]
sellToken(address _contract, uint256 tokenId, uint256 _quantity, uint256 _pricePerToken, address _payToken)  0xad1ee6be [COMPUTED]
withdrawListing(address op, uint256 tokenId, uint256 quantity)                              0x3e65bbba [COMPUTED]
createOffer(address _contract, uint256 tokenId, uint256 _quantity, uint256 _pricePerToken) payable           0xd898aaf2 [COMPUTED]
createOffer(address _contract, uint256 tokenId, uint256 _quantity, uint256 _pricePerToken, address payToken)  0xa86d2604 [COMPUTED]
acceptOffer(address from, address _contract, uint256 tokenId, uint256 _quantity)            0xf190078e [COMPUTED]
cancelOffer(address _contract, uint256 tokenId)                                             0x058a56ac [COMPUTED]
```
**EVENTS:** `ItemListed / ItemSold / ItemUnlisted / OfferAccepted / OfferCanceled / OfferSettled / PaymentLog`.

### 1.3 Operative — per-asset ERC-1155 (OperativeBuyable / OperativeBuyableSellable)
ABI: `OperativeBuyable.json` / `OperativeBuyableSellable.json` / `IOperative.json`.

**Access-token / role model (sub-token ids INSIDE the operative — NOT the content-hash tokenId):**
```
ACCESS_TOKEN()       -> uint256 == 1     // the entitlement to consume. sellersOf/listings/balanceOf queried at id=1.
ROYALTY_SHARE()      -> uint256 == 2     // the resale + royalty token, traded on TradeGateway.
DISTRIBUTION_RIGHT() -> uint256 == 3     // distribution shares.
```
> PC2 confirms (`wallet.js` lines 109-111): `TOKEN_ID_ACCESS=1`, `TOKEN_ID_ROYALTY_SHARE=2`, `TOKEN_ID_DISTRIBUTION=3`. The indexer **always** queries `sellersOf`/`listings` at `id=1`, never at the `AssetCreated` content tokenId.

**OP_TYPE tiers** (`OP_TYPE() -> uint16`): `0 = FREE`, `1 = BUY_ONCE` (stream/download single-buy), `2 = BUY_AND_RESELL`. FREE ⇒ no listing, no royalty market, no buy. (Note: `elacity-web` also documents an `OP_TYPE` semantic of `1=stream / 2=download / 0=free`; PC2's `0/1/2 = free/buy_once/buy_and_resell` is the catalog tiering. Treat `OP_TYPE` as an opaque uint16 you surface; do not branch on it beyond `==0 ⇒ free/no-market`.)

**Other reads/writes:**
```
paymentProcessor() -> address           0xf1c6bdf8 [COMPUTED]   // ERC20 approval target for BUY
checkAccess(address) -> tuple[]
royaltyInfo(uint256 salePrice) -> (address receiver, uint256 amount)[]
resellerCut() -> uint16                 // bps, OperativeBuyableSellable only
hasTradeAccess(address, uint256) -> bool
contentId() -> bytes16
balanceOf(address, uint256) -> uint256
isApprovedForAll(address,address) -> bool   0xe985e9c5
setApprovalForAll(address,bool)             0xa22cb465
safeTransferFrom(...) ; withdrawRewards(address payToken)
```

### 1.4 DigitalAssetLedger / DigitalAsset (ERC-721 channel ledger) — publish/mint
**On Base there is NO single global ledger.** Ledgers are **per-channel**, enumerated from the `.channels` collection. `DIGITAL_ASSET_LEDGER` is intentionally omitted from the Base map.
```
mint(address authority, string uri, uint16 opType, bytes opRawData, bytes sellRawData)   // DigitalAssetLedger
mint(string _uri, uint16 opType, bytes opRawData, bytes sellRawData) payable              // DigitalAsset (channel variant)
```
Mint encoding (from `elacity-web/src/lib/drm/utils.ts`, mirrored by `chain-provider::assemble_mint`):
- `opRawData = abi.encode(['bytes16','string','address[]','uint256[]','uint256[]', ('uint16' if resellable)], opArgs)` — **leads with `bytes16 contentId`**.
- `sellRawData = abi.encode(['uint256','uint256','address'], [copies, parseUnits(pricePerSale), payToken])`.
- `opType`: `0=FREE / 1=BUY_ONCE / 2=BUY_AND_RESELL`; DistributionRight publish uses type `3`.

### 1.5 CoreStorage — registry / fees
```
getListing / getOffer / listings / offers / sellersOf / offerersOf
operator(channel, tokenId) -> address
ipReference(bytes16) -> (address, uint256)
taxInformation() -> (uint16 platformFee, address)
protocolShares()
registerDigitalAsset(...)
```
Events: `IPBound / ChannelBound / ContractAcknowledged`.

### 1.6 Legacy ESC NFT Marketplace + Auction — DO NOT BIND ON BASE
`MARKETPLACE_ADDRESS`, `AUCTION_ADDRESS`, `FACTORY_ADDRESS` (`salesMixin.ts`, `src/components/marketplace/*`) are a **separate** fixed-price+auction ERC-721/1155 system wired only for ESC chains **20/21**. Base 8453 ships **only** the dDRM AuthorityGateway/TradeGateway stack. The "Auction" role exists **only in the legacy ESC system** — there is no auction surface on Base.

---

## 2. Addresses + chain ids to pin

### 2.1 Base mainnet — chainId **8453** (`0x2105`), RPC `https://mainnet.base.org`
All three sources agree on these. **CONFIRMED** = identical in `elacity-web` `Ecosystem.tsx` (`release/base-network`), PC2 `wallet.js`+`config/default.json`, and this runtime.

| Role | Address | Status |
|------|---------|--------|
| **AuthorityGateway** (primary market, access oracle, license verifyingContract) | `0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D` | **CONFIRMED** (3/3 sources) |
| **TradeGateway** (secondary/royalty market) | `0xd02451BCE627EF476B8ee52Cf131C426f67dbcB2` | **CONFIRMED** (elacity-web + PC2 client) |
| **CoreStorage** / `central_storage` | `0x0C1EeA2A3361B80AC0e42179335dB536A951760b` | **CONFIRMED** (elacity-web + PC2 config) |
| **Channel factory** / `CHANNEL_CORE` | `0xE1365ed47353De2F8A6a69E271e36650A9EE368F` | **CONFIRMED** (3/3 sources) |
| **EventHub** (`event_hub`, v3 AssetCreated source) | `0x5a694A6d988354dca491fe0F6db7a6ef46b656c2` | **CONFIRMED** (PC2 config; not in runtime yet — ADD) |
| **UniversalCheckin** | `0x2361a02e6727Ff1798920186b8ACf0f100f621C0` | CONFIRMED (elacity-web) |
| **USDC** (default payToken, 6 decimals) | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | **CONFIRMED** (3/3 sources; canonical Base USDC) |
| **Native ETH** | `AddressZero` (`0x0000...0000`, 18 decimals) | CONFIRMED |
| **DigitalAssetLedger** | *(intentionally absent — per-channel, enumerate from `.channels`)* | CONFIRMED-ABSENT |
| Channel factory **deploy block** (eth_getLogs lower bound) | `43892000` | **CONFIRMED** (PC2 config + runtime `DEFAULT_CHANNEL_FROM_BLOCK`) |

> The **AuthorityGateway used at runtime for a specific asset's buy/list/license is read PER-ASSET** from the asset metadata (`tokenInfo.metadata.properties.authority`), **not** from the static map. The map address `0x09dBe7...` is the registry **default/fallback** (and `chain-provider`'s `DEFAULT_AUTHORITY_GATEWAY` when a channel's `authority()` read misses). Resolve per-asset first; fall back to `0x09dBe7...`.

### 2.2 Other chains (for the chain-provider table; mirror `marketplaceSupportedChainIds = [20,21,421614,8453]`)
- **ESC mainnet (20):** AuthorityGateway `0x3B2Ef1C0342d7844369C031f08FE152f90d558e9`, TradeGateway `0xDe239B63949948FaC2A21aaa39bE0cd4775b1763`, CoreStorage `0x8D66Efaf34958A48F8Fa371A9c2DbDFe3D692fBb`, DigitalAssetLedger `0x9057304A41919008d79B3Bb3fCEBd69414e38b1F`, ChannelCore `0x7B89a5E0728C0f15DDe1D85ed1baa2bEa7E38Da0`. (Legacy ESC market also present here only — §1.6.)
- **Arbitrum Sepolia (421614, testnet):** AuthorityGateway `0x5207439A56C16A6fFb02f1AF0321D79Cf037738f`, TradeGateway `0x308AB0599FCb255773959B994250B9A5b87Db689`, CoreStorage `0x961D93965EA749E1e0A9E96dde05E7C464c59a46`.
- **ESC testnet (21) / 1337 (local):** dev/test placeholders.
- `.env.example` (`REACT_APP_*`, chainId 3) = **stale legacy** — ignore.

### 2.3 RPC pools (Base)
- General pool: `mainnet.base.org`, `base-rpc.publicnode.com`, `base.drpc.org`, `blastapi`, `meowrpc`, `1rpc.io/base`, with a health tracker that sidelines 5xx/429/403.
- **`eth_getLogs` curated subset (load-bearing):** `[mainnet.base.org, base.gateway.tenderly.co]` only. **publicnode is EXCLUDED** — it silently truncates wide ranges. (`config.rs` `PC2_BASE_LOG_RPC_POOL`.)

---

## 3. Per-verb mapping (contract call + live reads + runtime seam change)

### 3.1 BUY (primary access) — `AuthorityGateway.buyAccess`
**Live reads first (in order):**
1. Resolve **operative** = `AuthorityGateway.operative(ledger, tokenId)` (or from asset metadata `.properties`).
2. Resolve **real `tokenId`** for the asset — the ledger content tokenId (NOT `SHA-256(content_id)`; see §4).
3. `sellersOf(operative, ACCESS_TOKEN=1)` → pick a seller.
4. `listings(operative, 1, seller)` → `(qty, pricePerToken, payToken)`; take the lowest `pricePerToken`. This gives **seller, price, payToken** — the bytes that must NOT be env-guessed.
5. If `payToken != AddressZero` (ERC20): `paymentProcessor = operative.paymentProcessor()`, then `allowance(buyer, paymentProcessor)`; if `< price*qty`, **assemble `approve(paymentProcessor, MaxUint256)` as a prepended leg**.

**Tx:** native → `buyAccess(seller, ledger, tokenId, qty, pricePerToken)` `0xf7580ad9`, `value = pricePerToken*qty`. ERC20 → `buyAccess(seller, ledger, tokenId, qty, pricePerToken, payToken)` `0x0ede2294`, `value=0`, after the approve leg.

**Confirm:** read back `AuthorityGateway.hasAccessByContentId(buyer, kid)` (do NOT trust a ledger flag — see §4 `owned_now`).

**Seam change (`buy_authority.rs`):**
- Replace ENV terms `ELASTOS_DDRM_BUY_SELLER/_LEDGER/_PRICE/_PAYTOKEN` with values from the `sellersOf`/`listings` live reads via `chain-provider` (these reads do not exist in chain-provider yet — add them, §3.4).
- Replace `word_from_id = SHA-256(content_id)` tokenId default with the real ledger tokenId resolver.
- **Implement the ERC20 approve leg** — currently `assemble_buy_tx` only sets `"requires_erc20_approve": true` and never assembles/broadcasts `approve`. PC2's Market portal **batches** approve+buy; the runtime must emit two transactions (approve then buy) or a batch.

### 3.2 LIST (primary access) — `AuthorityGateway.sellAccess`
**Live reads:** `operative.isApprovedForAll(seller, AuthorityGateway)`.
**Tx:** if not approved → `operative.setApprovalForAll(AuthorityGateway, true)` `0xa22cb465`; then `sellAccess(ledger, tokenId, quantity, parseUnits(price, decimals), payToken)` `0x9a3fa9f5`.
> **Approval target = the AuthorityGateway itself** (NOT paymentProcessor). Approving the wrong spender reverts.
**Seam change:** does not exist anywhere — build in the new marketplace assembler (§6).

### 3.3 CANCEL (primary) — `AuthorityGateway.withdrawListing`
**Live reads:** `listings(operative, 1, seller)` for current qty.
**Tx:** `withdrawListing(operative, tokenId, quantity)` `0x3e65bbba`. No approval.
**Seam change:** build in the new assembler (§6).

### 3.4 Secondary / royalty (TradeGateway)
- **buy:** `sellersOf/listings` on TradeGateway → ERC20 `approve(TradeGateway, price)` → `buyToken(seller, contract, tokenId, qty)` `0x7d17ff3d` (+`value` if native).
- **list:** `setApprovalForAll(TradeGateway, true)` → `sellToken(contract, ROYALTY_SHARE=2-bearing tokenId, qty, pricePerToken, payToken)` `0xad1ee6be`. **Approval target = TradeGateway.**
- **offer:** `approve(TradeGateway, price)` → `createOffer(...)` `0xd898aaf2` (native, `value`) / `0xa86d2604` (ERC20). `acceptOffer(from, contract, tokenId, qty)` `0xf190078e`. `cancelOffer(contract, tokenId)` `0x058a56ac`.
- **cancel listing:** `withdrawListing(contract, tokenId, qty)` `0x3e65bbba`.

**Seam change (`chain-provider`):** add typed read ops — `sellers_of(op, tokenId)`, `listings(op, tokenId, seller)`, `payment_processor(op)`, `allowance(token, owner, spender)`, `token_uri(channel, tokenId)`. These are plain `eth_call` encoders parallel to the existing `has_access_by_content_id`. No raw RPC passthrough to apps (preserve the typed-capability principle).

### 3.5 DISCOVER — see §5.

### 3.6 OPEN / access gate — `hasAccessByContentId` + EIP-712 license
**On-chain bool:** `AuthorityGateway.hasAccessByContentId(holder, bytes16 contentId)` `0x54d42821`, `eth_call latest`. **Fail CLOSED:** a contract revert ⇒ `false` (403). A genuine transport/RPC error ⇒ propagate (503) so an outage can't masquerade as a denial. (This is exactly what `chain-provider/main.rs:829` already does — keep it.)
**Decrypt/playback:** the EIP-712 license cert (domain `"AuthorityGateway"`, `protocolVersion`, `contentId bytes16`); the Lit Action runs `hasAccessByContentId(owner, kid)` on-chain.
> `kid` / `contentId` = the **bytes16** content id = `0x` + lowercase(`kid_hex[0:32]`). **No hash, no truncation beyond taking the metadata kid as-is.** `content-market` enforces `metadata.kid == calldata contentId` or errors `identity_mismatch`.

---

## 4. Phase 1 — the buy-invariant, restated with the REAL ABI

**Invariant (decision-resolved):** a live primary buy on Base is byte-correct **iff** every one of these is sourced from chain (not env, not a hash):

1. **method** — native `0xf7580ad9` xor ERC20 `0x0ede2294`, chosen by `payToken == AddressZero` (USDC default ⇒ ERC20 path is the default).
2. **arg order** — `(seller, ledger, tokenId, quantity, pricePerToken [, payToken])`. (This ordering is what `buy_authority.rs` hand-assembles today and flags as "the demo's documented default"; it is **correct** per the real ABI — keep the order, fix the *sources*.)
3. **seller** — from `sellersOf(operative, 1)` (NOT `ELASTOS_DDRM_BUY_SELLER`, NOT `= subject`).
4. **pricePerToken + payToken** — from `listings(operative, 1, seller)` (NOT `ELASTOS_DDRM_BUY_PRICE=0` / `_BUY_PAYTOKEN`).
5. **tokenId** — the **real ledger content tokenId**, NOT `word_from_id = SHA-256(content_id)` (which `buy_authority.rs:374` itself documents as "representative encoding only"). Until a resolver exists, `ELASTOS_DDRM_BUY_TOKEN_ID` must be pinned to the true id; **the real fix is to read it** (from `AssetCreated`/`DigitalAssetRegistered` data via content-market, keyed by the asset's `bytes16` kid).
6. **ledger** — the per-channel ledger address (NOT AuthorityGateway-as-ledger; `buy_authority.rs` already flags `_BUY_LEDGER` defaulting to `to` as wrong).
7. **value** — `pricePerToken * quantity` **only** on the native overload; `0` on ERC20.
8. **approve leg** — ERC20 path MUST prepend `approve(operative.paymentProcessor(), price)` (selector `0x095ea7b3`). **Unimplemented today — build it.**
9. **confirm** — `owned_now` for the live `chain` mode is read back from `hasAccessByContentId`, **not** from a local ledger. (dev/chain-mock set `owned_now=true` from the ledger; live chain sets it from chain. Any caller treating `owned_now` as "confirmed" must use the chain mode.)

**Word layout (no ABI lib; manual 32-byte concat, matches `buy_authority.rs`):**
```
selector ‖ leftpad32(seller) ‖ leftpad32(ledger) ‖ tokenId(32) ‖ leftpad32(quantity) ‖ leftpad32(pricePerToken) [‖ leftpad32(payToken)]
```

---

## 5. Phase 2 — the index (real event topics + listing schema)

The runtime model is **synchronous pull only** (`eth_getLogs`), no `eth_subscribe`/websocket/filter-polling anywhere. Constraints to honor:
- `DEFAULT_MAX_LOG_RANGE = 10_000` blocks (env `ELASTOS_CHANNEL_MAX_LOG_RANGE`), `MIN_LOG_RANGE = 2_000` floor, adaptive halving on range errors.
- log-RPC curated subset only (`mainnet.base.org` + tenderly; §2.3).
- lower bound = block `43892000`.
- persisted resumable cursor (forward head + backfill floor); a selected channel re-confirmed on-chain before any mint.

### 5.1 Topics to `getLogs` (CONFIRMED — pinned identically in PC2 indexer + this runtime)
```
ChannelCreated           0x4ae6ef95ddade103ca67593cd4cf68dda177aa1054ad4eeb4963d2c3df44702e   on channel factory
  sig: ChannelCreated(uint8 indexed channelType, uint8 indexed scope, address indexed creator, address channel, address factoryAddr)
  channel = data word[0]; creator = topics[3].

AssetCreated (v3)        0xc0a995e4052be044599af577ab2f3382d67bd34df95a76226e7c464e9d4dba46   on event_hub (fallback central_storage)
  sig: AssetCreated(address indexed _to, address indexed _channel, uint256 _tokenId, string _tokenUri, uint16 _opType, address indexed opContract)
  topics[1]=creator, topics[2]=channel, topics[3]=opContract(=operative); data=abi.encode(uint256 tokenId, string tokenUri, uint16 opType).
  Carries NO contentId -> metadata_status:"needs_kid" (identity resolved later by kid-match).

DigitalAssetRegistered (legacy + v3 identity)   0x1b24f7763272894608506beba5887c374d345cd231bf52bd03f40bc2d0508d7b
  sig: DigitalAssetRegistered(address indexed channel, uint256 indexed tokenId, address creator, string tokenURI, uint16 opType, bytes16 contentId)
  topics[1]=channel, topics[2]=tokenId(hex); data carries the bytes16 contentId -> identity COMPLETE.
```
> Store uint256/hash tokenIds as **hex strings** (avoid JS/serde number overflow). `op_type_code 0/1/2 -> free/buy_once/buy_and_resell`. `content_id` rule: `bytes16 == 0x + lowercase(kid_hex[32])`.

### 5.2 Listing-lifecycle events (✅ topic0 CONFIRMED live 2026-06-23 — see §1.1 EVENTS)
`ItemListed`/`ItemSold` topic0 are now confirmed against deployed AuthorityGateway logs (`ItemListed 0x90aecdd7…66d2`, `ItemSold 0x60cd9eee…a955`); `ItemUnlisted 0xdb6bedce…b415` is keccak-correct (on-chain sample pending). The TradeGateway `Offer*` set is still unconfirmed. PC2 reconstructs prices by **polling `sellersOf`+`listings` every detail-view open (30s cache)**, not from events. Two viable index designs:
- **(A) Poll model (matches PC2, lowest risk):** every scan cycle, for each paid asset (`op_type>0`, non-zero operative), `eth_call sellersOf(operative,1)` then `listings(operative,1,seller)` per seller; take the lowest `pricePerToken`+`payToken`. No new topics needed. **Recommended for Phase 2.**
- **(B) Event model:** add `ItemListed/ItemSold/ItemUnlisted` to the `getLogs` topic set — **requires computing+confirming their topic0 against a deployed log first** (§7-G).

### 5.3 Listing schema (`ContentListingV1`, extend content-market's decode output)
```
content_id        : bytes16 (0x + lowercase kid[32])
channel           : address
operative         : address           // AuthorityGateway.operative(ledger, tokenId)
ledger            : address           // per-channel ledger
token_id          : hex string        // real ledger content tokenId
op_type           : uint16            // 0 free / 1 buy_once / 2 buy_and_resell
token_uri         : string            // from event data or tokenURI(tokenId) 0xc87b56dd
metadata          : { name, description, image|media.previewURL, media.uri->content_cid, contentType->asset_type, kid }
metadata_status   : "needs_kid" | "resolved" | "identity_mismatch"
# live market fields (from §5.2-A):
sellers           : address[]         // sellersOf(operative, 1)
price             : uint256 (lowest)  // listings(...).pricePerToken
payment_token     : address           // listings(...).payToken (USDC or AddressZero)
quantity          : uint256
# secondary (TradeGateway), optional:
royalty_listings  : [{ seller, price, payToken, qty }]   // sellersOf/listings on TradeGateway at ROYALTY_SHARE=2
```

---

## 6. Order-assembly for LIST + CANCEL (real selectors) — the missing capsule

**BUILT (gateway-side):** the `sellAccess` / `withdrawListing` / `setApprovalForAll` surface is implemented as a **pure, selector-pinned calldata assembler** in `elastos/crates/elastos-server/src/api/trade_authority.rs` (read → pure-encode → wallet sign → `chain-provider.broadcast_transaction`; never holds keys/RPC inline) and wired to `/api/market/order/{sell,withdraw,approve}`. Still unbuilt: the `sell*Token` / `*Offer` TradeGateway royalty-share surface.

**Primary market (AuthorityGateway):**
```
LIST   sellAccess(address ledger, uint256 tokenId, uint256 quantity, uint256 pricePerToken, address payToken)
       selector 0x9a3fa9f5
       words: ledger ‖ tokenId ‖ quantity ‖ pricePerToken ‖ payToken
       PRE: if !isApprovedForAll(seller, AuthorityGateway) -> setApprovalForAll(AuthorityGateway, true)  0xa22cb465 (target = AuthorityGateway)

CANCEL withdrawListing(address operative, uint256 tokenId, uint256 quantity)
       selector 0x3e65bbba
       words: operative ‖ tokenId ‖ quantity
       PRE: none. (read listings() for current qty)
```
**Secondary market (TradeGateway):**
```
LIST   sellToken(address _contract, uint256 tokenId, uint256 _quantity, uint256 _pricePerToken, address _payToken)   0xad1ee6be
       PRE: setApprovalForAll(TradeGateway, true)  (target = TradeGateway)
CANCEL withdrawListing(address op, uint256 tokenId, uint256 quantity)   0x3e65bbba
OFFER  createOffer(address,uint256,uint256,uint256) 0xd898aaf2 (native, value) | createOffer(...,address) 0xa86d2604 (ERC20)
       acceptOffer(address from,address _contract,uint256 tokenId,uint256 _quantity) 0xf190078e
       cancelOffer(address _contract,uint256 tokenId) 0x058a56ac
```
> All `[COMPUTED]` selectors above were keccak-derived from the verified ABI signatures in `wallet.js`/`*.json`. **The two whose signature differs by a single comma will collide-check against a deployed contract — confirm `sellAccess`/`sellToken`/`withdrawListing`/`createOffer` selectors against the real bytecode (or an etherscan ABI) before first mainnet write** (§7-F).
> Keep the runtime principle: **selectors supplied as operator-config defaults, never keccak'd in-capsule.** Pin them in `config/default.json` like the existing `buy_authority` selectors, overridable via env.

---

## 7. Honest gaps, risks, and things to verify

**A. `hasAccessByContentId` — two contradictory declarations in PC2.** RESOLVED in favor of the AuthorityGateway form:
- `storage.ts:2771` + this runtime (`abi.rs`, selector `0x54d42821`, "confirmed against `~/.pc2 contracts/abis.ts`"): `hasAccessByContentId(address holder, bytes16 contentId)` on AuthorityGateway `0x09dBe7...`. ✅ **Use this.**
- `ChannelBridge.ts:508-513`: `hasAccessByContentId(bytes32 contentId, address user)` on registry `0x96826e93c4b0bb9D4dFCcb080bFe6E05cC363e36` — **arg order, types (bytes32 vs bytes16), AND target all differ.** This path is suspect; do not bind to it. **TODO:** treat `ChannelBridge`'s variant as a PC2 bug to fix, not a second valid ABI. The runtime already uses the correct `0x54d42821`/`address,bytes16` form.

**B. tokenId resolver missing.** No code maps a `bytes16` kid → the real ledger content tokenId. Phase 1 needs this (today it falls back to `SHA-256(content_id)`, which is wrong). Source of truth = `AssetCreated`/`DigitalAssetRegistered` event data, joined on the asset's kid. Until built, pin `ELASTOS_DDRM_BUY_TOKEN_ID`.

**C. ERC20 approve leg unimplemented in `buy_authority.rs`.** Default path is USDC (ERC20). The buy will revert without a prior `approve(paymentProcessor, price)`. Build it (§3.1/§4-8).

**D. List/cancel/offer surface entirely absent.** No contract binding, no assembler, no address wiring beyond AuthorityGateway. §6 is greenfield.

**E. Listing-lifecycle event topics — ✅ RESOLVED (live pass 2026-06-23).** `ItemListed`/`ItemSold` topic0 confirmed against deployed logs; `ItemUnlisted` keccak-correct (sample pending); the TradeGateway `Offer*` set is still unverified. The Phase-2 **poll model (§5.2-A)** still needs none of them; the event-driven `/api/market/listed`+`/history` index can now be built off the confirmed AuthorityGateway topics (`Offer*` excepted).

**F. `[COMPUTED]` write selectors need on-chain confirmation.** `buyAccess` (`0xf7580ad9`/`0x0ede2294`), `sellersOf` (`0x997eab2d`), `listings` (`0x6bd3a64b`), `setApprovalForAll` (`0xa22cb465`), `hasAccessByContentId` (`0x54d42821`) are **triple-source confirmed**. The rest (`sellAccess 0x9a3fa9f5`, `withdrawListing 0x3e65bbba`, `sellToken 0xad1ee6be`, `buyToken 0x7d17ff3d`, `createOffer 0xd898aaf2/0xa86d2604`, `acceptOffer 0xf190078e`, `cancelOffer 0x058a56ac`, `paymentProcessor 0xf1c6bdf8`, `hasAccess 0xcf56b4eb`) are derived from verified ABI strings but **not yet seen pinned in-source** — confirm against the deployed bytecode / a verified explorer ABI before the first mainnet write.

**G. Per-asset authority resolution.** Don't assume `0x09dBe7...` for every asset's buy/list/license. Read `metadata.properties.authority` first; the static address is the fallback only.

**H. Contract-version risk.** Two generations coexist: legacy `DigitalAssetRegistered` (CentralStorage, numeric tokenIds) and current v3 (`EventHub` + `AssetCreated`, 256-bit hash tokenIds, Operative). The indexer must remain **version-keyed** (`content_indexer.contracts.v3`) so a future v4 is a config entry. All contracts are upgradeable proxies (`initialize()` + AccessControl) — addresses are stable but **implementation/ABI can change behind the proxy**; re-verify selectors after any announced upgrade.

**I. `EventHub` not yet in the runtime.** PC2 scans `AssetCreated` on `event_hub 0x5a694A6d...` (fallback CentralStorage). The runtime's `content-market`/`chain-provider` should add `event_hub` as the primary v3 `AssetCreated` source.

**J. SubscriptionModule (out of scope here, flagged for completeness).** PC2 references a SubscriptionModule (`bulkUpdatePlans` tuple `(uint8 actionType, bytes args)`, `PlanActionType ADD=1/UPDATE=2/REMOVE=3`, `subscribePlan(uint8,bytes)`, `getPlans/plans`). Not part of buy/list/cancel; cross-check `elacity-web/src/lib/drm/channel/{subscription.ts,subscribe.ts}` if subscriptions are added.

---

## 8. Source-of-truth file map (all absolute)
**elacity-web (`release/base-network`):** `src/lib/drm/contracts/{AuthorityGateway,TradeGateway,DigitalAsset,DigitalAssetLedger,CoreStorage,OperativeBuyable,OperativeBuyableSellable,IOperative}.json`; `src/lib/web3/Ecosystem.tsx`; `src/lib/web3/network/constants.ts`; `src/components/Cinema/Media/MediaContext.tsx` (buy/list); `src/components/Cinema/Governance/contexts/GovernanceActionContext.tsx` (royalty/offers/cancel); `src/lib/web3/executable/executors/eip1193/tx.ts`; `src/hooks/usePlayerCertificate.tsx`; `src/lib/drm/license/request.ts`; `src/lib/drm/utils.ts`; `src/constants/contract.ts`.
**PC2 (`pc2.net/pc2-node`):** `data/installed-apps/elacity-market/wallet.js` (canonical client ABI + selectors); `src/services/ContentIndexerService.ts` (topics + read selectors); `data/installed-apps/elacity-market/api.js` (`catalogItemToNft` adapter); `config/default.json` (`content_indexer.contracts.v3`); `src/api/storage.ts` (`hasAccessByContentIdWithFailover`, the correct ABI); `src/services/gateway/ChannelBridge.ts` (the suspect ABI — §7-A); `src/utils/rpc.ts` (RPC health tracker).
**This runtime (`feat/marketplace-runtime` / `feat/ddrm-hardening-and-creator-parity`):** `elastos/crates/elastos-server/src/api/{buy_authority.rs,trade_authority.rs,content_index.rs,gateway_marketplace.rs}` (money-path + resale assembler + discovery + routes — all built gateway-side); `capsules/chain-provider/src/{main.rs,abi.rs,config.rs,channel_index.rs}` (KID→tokenId resolver §6); `capsules/content-market/src/main.rs`; `capsules/marketplace-content/browser/` (UI shell, no authority); `capsules/marketplace/src` (legacy app-store wasm shell — UI only, not the assembler).
