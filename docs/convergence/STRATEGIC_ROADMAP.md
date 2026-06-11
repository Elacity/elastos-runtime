# ElastOS ⇄ PC2 dDRM — Strategic Roadmap & Sequencing

> **Audience:** the convergence team + stakeholders. **Purpose:** one place that answers
> *where are we, what's left, and in what order/timing* — for the two **portals** (Elacity
> **Create** + **Market**), the **base.ela.city / Lit** legacy path, and the **distributed
> PQ dKMS** node deployment. Companion to `CONVERGENCE_PLAYBOOK.md` (how), `PRODUCT_VISION.md`
> (why), `CONVERGENCE_AUDIT.md` (pipeline status), `ELASTOS_ARCHITECTURE_VISUAL.md` (visual).
>
> **Grounding:** this is a *real source audit* of the PC2 repo at `~/.pc2`
> (`github.com/Elacity/pc2.net`, `v1.2.7.12`), not just our notes.

---

## 0. North star (unchanged)

> *Download and run a video I own — like I can from the Elacity Market on PC2 today —
> inside the ElastOS Runtime, with the key custody owned by us (PQ‑hybrid dKMS), never Lit.*

There are **two ways to reach it**, and they set the sequencing (see §6, the one fork):
- **Bridge** the *existing* Lit‑escrowed catalog (fast, but depends on Lit).
- **Native** PQ end‑to‑end: **Create** a new asset on our rail and consume it (product truth).

---

## 1. What PC2 actually is (so we copy patterns, not plumbing)

| PC2 piece (`~/.pc2`) | What it does | Runtime counterpart |
|---|---|---|
| `packages/access` = `@elacity-js/access` | the dDRM **SDK**: `verifyAccess` (chain read, no key) · `acquireKey` (Lit) · `encryptBuffer` (creator) · `decryptBuffer` · `fetchAndDecrypt` | the **provider chain** `rights → key → decrypt` + `encrypt-provider` |
| `packages/access/src/lit/*` | CEK sealed to **Lit** under `ERC1155.balanceOf(user,tokenId)>0`; SIWE `sessionSigs` → `client.decrypt()` | **`key-provider`** (PQ dKMS is product truth; Lit = one optional backend) |
| `packages/access/src/contracts/abis.ts` | real **Base** ABIs + addresses (V3 contract system) | **`chain-provider`** typed reads + calldata |
| `pc2-node/crates/{cenc-decrypt,cenc-encrypt,mp4-split,evm-multicall,ipfs-assemble}` | the Rust/WASM dDRM internals | already mirrored as **provider internals** (`decrypt-provider`, `encrypt-provider`, `ddrm-media`, `chain-provider`, `ipfs-provider`) |
| `src/backend/apps/*` (Puter apps) | the **portal UIs** (Create / Market / app-center / dao-dashboard) served by the Puter GUI | **app‑tier capsules** (web + signed manifest, de‑privileged) — *not yet built* |
| `ElacityLabsWeb` (`~/Projects`) | the **marketing** site (blog/SEO), not the dDRM portal | n/a |

**The crucial insight:** `@elacity-js/access` was *designed* to be re‑hosted behind a
capability runtime — its own comment says the steps are separated "so the Runtime can
insert capability token issuance." We are not fighting PC2's design; we are completing it.

---

## 2. Real contract truth (corrects two runtime placeholders)

From `~/.pc2/packages/access/src/contracts/abis.ts` (Base, chain id 8453):

- **Access read (rights gate):** `AuthorityGateway.hasAccessByContentId(address holder, bytes16 contentId) → bool`.
  - ⚠️ **Correction:** our `chain-provider` currently encodes a *guessed* `(string contentId,
    address subject, string right)` shape. The real ABI is `(address holder, bytes16 contentId)`.
    Aligning this is cheap and de‑risks the whole rights path.
- **Mint:** `DigitalAsset.mint(string _uri, uint16 opType, bytes opRawData, bytes sellRawData) payable`.
  - ✅ Our `publish-provider` / `chain-provider.assemble_mint` already match this shape.
  - `opType ∈ {FREE:0, BUY_ONCE:1, BUY_AND_RESELL:2}`; roles `{ACCESS_TOKEN:1, ROYALTY_SHARE:2, DISTRIBUTION_RIGHT:3}`.
- **Buy:** there is **no `buyAccess`**. Purchase flows through the **operative**:
  `AuthorityGateway.operative(channel, tokenId) → operative`, then the operative's
  `paymentProcessor()` (ERC‑1155 `setApprovalForAll`/balance semantics, USDC on Base).
  - ⚠️ **Correction:** our `buy_authority` selector/calldata is a documented placeholder.
    The real path is "resolve operative → call its purchase entrypoint."
- **Permissionless mint:** `PUBLIC_ELACITY_CHANNEL` lets any wallet mint without deploying a
  channel — the **fast path for a first Create demo**.
- Real addresses are pinned in `abis.ts` (AuthorityGateway `0x09dBe…`, ChannelFactory,
  AssetFactory, RoyaltyTradeGateway, CentralStorage, EventHub, USDC `0x8335…`).

---

## 3. Where we are (crown‑jewel loop, by reality)

```
Create(package+encrypt+mint) → Market(list+buy) → rights → key/dKMS → decrypt → render
        🟡 backend                  🟡 backend        🟢*      🟢built     🟢        🟢 media
        🔴 portal UI                🔴 portal UI               🔴deployed            🔴 non-media
```

- 🟢 **Decrypt + media playback** real in Home; **non‑media viewer** (`ddrm-viewer`) is the active capability gap.
- 🟢\* **Rights gate** drives the real `chain-provider` read — *but on the guessed ABI shape* (§2).
- 🟢 **PQ dKMS** built + proven across 3+ nodes locally over TCP; **🔴 not deployed off‑box** (the "make it real" milestone).
- 🟡 **Buy**: assemble→**sign (real, via `wallet-provider`)**→broadcast→record→re‑check wired; real **operative** purchase path not yet.
- 🟡 **Mint**: faithful calldata; **live broadcast now unblocked** by the `wallet_signer` rail (just shipped) — not yet wired.
- 🔴 **Create portal** and 🔴 **Market portal** as runtime capsules: not built. (`capsules/marketplace` is an *app‑store*, not the Elacity content market.)
- 🟡 **Lit compat**: `key-provider` has an operator‑selectable `lit` slot; adapter not shipped.

---

## 4. The sequenced roadmap (with timing + the "why now")

Two tracks run **in parallel**: **make it REAL** (protocol/infra) and **make it USABLE** (portals).

### Track A — make it REAL (do first / start now)

1. **Live mint via `wallet_signer`** *(now, 1 slice).* The signing rail just shipped; mint is
   the same rail. Closes the **Create backend** so an asset actually lands on Base. Highest
   leverage, lowest effort.
2. **Align `chain-provider` to the real Base ABIs** *(now, 1 slice).* Swap the guessed
   `hasAccessByContentId` shape for `(address holder, bytes16 contentId)`; wire the
   **operative/paymentProcessor** buy path; pin the real addresses from `abis.ts`. Replaces
   placeholders with truth and de‑risks everything downstream.
3. **dKMS 3‑node deployment** *(start NOW — long lead time; runs in parallel).* Stand the PQ
   threshold daemons up on **interserver + contabo + a 3rd** node (you can spin the 3rd when
   ready). This is the **headline "the quantum CEK is real and owned by us"** milestone and
   the thing the crown jewel ultimately rests on. The crypto is done; this is **ops**:
   provisioning, TLS, firewall, supervised daemons, the DKG/key ceremony, then flip
   `key-provider` from local → deployed quorum. Start provisioning early because infra has
   the longest lead time and it blocks nothing else.

### Track B — make it USABLE (portals, after A1–A2 land)

4. **Create portal capsule** *(2–4 wk).* Package → `encrypt-provider` → `publish/chain` →
   `wallet_signer` mint. Mint to `PUBLIC_ELACITY_CHANNEL` first (permissionless). *Build this
   before Market — there's nothing to sell until Create works.*
5. **Market portal capsule** *(2–4 wk).* `content-market` listings via live `eth_getLogs` →
   buy via the real operative path → click‑to‑open (reuses today's open/auto‑buy rail).

### Opportunistic — gated on the §6 product fork

6. **Lit‑compat adapter** behind `key-provider` *(1–2 slices).* Replicate `acquireKey()`
   (SIWE `sessionSigs` → Lit `decrypt`) so the runtime can play **legacy** Lit‑escrowed assets
   from the *current* base.ela.city catalog. No app/decrypt change. This is the **only** path
   to the north star against **today's** assets.

---

## 5. base.ela.city, the browser, and Lit — the honest picture

- **base.ela.city** = the live Elacity dDRM web app (PC2 backend `apps/*` + `@elacity-js/access`
  + Lit). The runtime's job is **not** to rebuild it — it's to **consume its assets through our
  native provider chain** (and offer Create/Market as sovereign capsules).
- The **runtime browser** (`capsules/browser`) can already navigate to base.ela.city as a web
  page today; that's orthogonal to dDRM. The dDRM value is in the **providers**, not the browser.
- **Lit is compatibility, not product truth.** Existing assets are sealed to Lit conditions; new
  assets seal to our **PQ‑hybrid dKMS**. We keep one Lit adapter as a bridge and never let it
  become the root of trust (`PRINCIPLES.md` #15, `CONVERGENCE_AUDIT.md` §4/§6).

---

## 6. The one fork to decide (drives whether step 6 jumps the queue)

- **Path A — Bridge the live catalog (Lit adapter first).** Fastest way to literally "play a
  video I already own from Elacity Market" inside the runtime. Pro: demoable against real,
  existing content now. Con: leans on Lit (not our PQ root); media still needs the EIP‑712
  license path.
- **Path B — Native PQ end‑to‑end (Create → mint → buy → PQ‑dKMS decrypt).** Proves the owned,
  quantum‑resistant spine the whole project exists for. Pro: product truth, sovereignty. Con:
  needs the Create portal before there's inventory.

**Recommendation:** spine = **Path B**, with **dKMS deployment started in parallel now** (longest
lead time) and **Path A kept as a quick bridge** if a near‑term demo against the live catalog is
wanted. Either way, A1 (live mint) + A2 (real ABIs) land first because they're cheap and unblock
everything.

---

## 7. Infra note — the 3‑node PQ dKMS

- **Have:** interserver + contabo. **Need:** a 3rd node (spin when ready) → a **2‑of‑3** quorum.
- **Why 3:** the dKMS is built for `2-of-2` and `2-of-3`; 2‑of‑3 gives fault tolerance (one node
  can be down/rotated without losing custody) and no single node ever reconstructs the CEK.
- **Work = ops, not crypto:** provision hosts, lock down the authenticated PQ channel over TCP,
  TLS + firewall, run the daemons under supervision, perform the DKG ceremony, record the quorum
  attestation, then point `key-provider` at the deployed endpoints. Then the CEK custody behind
  every decrypt is genuinely distributed and owned by us.

---

*Living document. Update as A1/A2 land, the portals ship, dKMS goes multi‑node, and the Lit
bridge decision is made.*
