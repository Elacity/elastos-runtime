# ElastOS ⇄ PC2 dDRM Convergence — Audit & Roadmap

> **Purpose:** a single, honest, bird's-eye answer to "where are we, what's built, what's
> left, and how do we get to the full creator → package → sign → IPFS/chain → buy/trade →
> download → run loop for **both media and non-media** assets — as the superior,
> ElastOS-native version of PC2."
>
> Read alongside `PRODUCT_VISION.md` (the why), `CONVERGENCE_PLAYBOOK.md` (the how),
> `SYSTEM_ARCHITECTURE_MAP.md` (the detailed map), `DDRM_STATUS.md` (the day-by-day ladder),
> and `PRINCIPLES.md` (the constitution). Where those disagree, `DDRM_STATUS.md` is the most
> current on crypto state and this doc is the most current on product-loop state.
>
> **Date:** 2026-06-11 · **Branch:** `feat/ddrm-home-playback`

---

## 0. TL;DR

The **cryptographic engine and provider contracts are essentially built and are genuinely
superior to PC2** — born-distributed threshold keys, PQ-hybrid sealing, in-VM decrypt with
CEK containment, IPFS/Helia-byte-compatible content addressing, on-chain mint calldata, a
marketplace indexer, and (as of 2026-06-11) **real video playback inside Home**. What is
**not** done is the **product skin and the live plumbing** around that engine: a
creator/packaging UI, the live wallet-signed mint broadcast, live IPFS pinning, the
buy/trade purchase flow, a **non-media viewer** (documents/images/3D), a shipped **Lit
compat adapter**, and **production multi-node dKMS deployment**.

**The hard part (the trustworthy core) is done. The remaining work is wiring it to
chain/IPFS/UI for real, and adding the surfaces.**

---

## 1. The full lifecycle, color-coded by reality

🟢 done/proven · 🟡 partial · 🔴 gap

| # | Stage | Capsule(s) | State | What's real | What's left |
|---|---|---|---|---|---|
| 1 | Encrypt / CENC seal | `encrypt-provider`, `ddrm-envelope` | 🟢 | CEK minted in-VM, CENC AES-128-CTR fMP4, escrow envelope, PQ-hybrid | feed real creator bytes |
| 1 | Content addressing | importer (`ddrm-runtime-open`), `ipfs-provider` | 🟢 | CIDv1 raw leaves + dag-pb balanced tree, **byte-identical to Helia**, fail-closed | live Kubo pin/serve at scale |
| 1 | Packaging (non-media) | — | 🔴 | decrypt is content-agnostic | render-tier packaging for pdf/img/epub/3D |
| 2 | Mint intent + calldata | `publish-provider`, `chain-provider` | 🟢 | `UnsignedMintV1`, PC2-faithful `mint()` ABI calldata, payee/royalty arrays | — |
| 2 | Wallet sign + broadcast | `wallet-provider`, `chain-provider` | 🔴 | signing + `eth_sendRawTransaction` exist | live assemble→sign→broadcast flow |
| 2 | IPFS pin | `ipfs-provider` | 🟡 | contract + addressing | live pin wired into publish |
| 3 | Marketplace / discovery | `content-market` | 🟢 (offline) | listing from calldata + chain event + metadata | live `eth_getLogs` |
| 3 | Buy / trade access | `wallet-provider`, `chain-provider` | 🔴 | ownership *check* is real | the *purchase* flow (buyAccess UI + tx) |
| 4 | Rights check | `rights-provider`, `chain-provider` | 🟢 | real `has_access_by_content_id` (owned→open, not-owned→fail-closed) | — |
| 4 | Key authority (dKMS) | `key-provider`, `dkms-authority` | 🟢 built | born-distributed DKG, 2-of-2 + 2-of-3, rotatable/reconfigurable, authenticated PQ channel over TCP, quorum attestation | **production multi-node deployment** (ops) |
| 4 | Lit compat backend | `key-provider` | 🟡 seam-only | operator-selectable slot, fails closed honestly | ship a Lit proxy adapter |
| 4 | Decrypt boundary | `decrypt-provider` | 🟢 | in-VM unwrap, multi-segment, single+threshold+quorum rails, `Zeroizing` | — |
| 4 | Media viewer | `elacity-player` | 🟢 **playing** | MSE segment streaming, fails closed on key fields | swap test clip → real owned title |
| 4 | Non-media viewer | — | 🔴 | — | `ddrm-viewer` render-tier capsule (PC2 `wasm-renderer`) + a 3D tier |
| — | Creator app UI | `library`/new | 🔴 | — | the "package & publish" surface |
| — | Orchestration host | `ddrm-plan-runner` (`DrmHost`) | 🟢 | composition root, capability table, fail-closed | fold into product gateway |

**Engine ≈ 90–97% done & verified; product loop (UI + live chain/IPFS + non-media + node
deployment) is the remaining real work.**

---

## 2. The Carrier thesis (and where we actually stand)

**Your understanding is correct and it is the core thesis:** sandboxed environments (microVM
/ WASM / frame) connected through a **Carrier capability plane**, gated by **capability
tokens**. This is `PRINCIPLES.md` #4 ("Carrier Plane For Local And Off-Box") and the product
vision verbatim.

**The one refinement:** "Carrier" is the **contract shape**, not a specific wire. Per
`PRINCIPLES.md` #4 and `CARRIER.md`, a capsule speaks **one capability-plane contract** — a
signed capability envelope authorized by a capability token — and **never names its own
transport**. The transport beneath is swappable **adapter plumbing**: in-process / shared
gossip buffer locally, stdio / vsock / loopback HTTP for adapters, **iroh P2P** off-box. Two
capsules on the same machine talk in-process (instant, no network) and that is still
"Carrier-shaped," not a violation.

> One-liner: **one Carrier-shaped capability plane, transport-agnostic — in-process locally,
> iroh P2P remotely, swappable underneath without any capsule code change.** Not "everything
> physically rides the P2P wire."

**Conformance grade today:**

| Hop | Carrier-shaped contract? | Transport today | Verdict |
|---|---|---|---|
| browser capsule → provider | ✅ capability call + Home token | loopback HTTP / `carrier_invoke` rail | ✅ compliant adapter |
| provider → off-box provider | ✅ | iroh P2P | ✅ |
| key-provider → dKMS node | ✅ endpoint descriptor | Unix socket / `tcp:` | ✅ permitted adapter (#4) |
| media-authority (today's demo) | ⚠️ **names its transport** | gateway spawns stdio subprocess | 🟡 valid adapter, **not yet lifted behind the unified plane** |

**Honest gap:** a few rails (the dKMS socket, the media-authority subprocess) are *valid host
adapters* but the gateway still *names the transport* instead of receiving a transport-blind
capability handoff. Lifting those behind the unified Carrier-capability plane (capsule never
names a transport) is a tracked roadmap slice — **not a correctness hole**, a maturity step.

---

## 3. You vs PC2 — where the "superior version" already is

| Dimension | PC2 (net repo) | ElastOS Runtime | Verdict |
|---|---|---|---|
| Key custody | Lit PKP threshold (opaque, off-platform) | **Owned born-distributed PQ dKMS**, you run the nodes, inspectable | ✅ superior |
| Crypto root | P-256 / ECDSA (classical) | **PQ-hybrid** (x25519‖ML-KEM-768, ML-DSA-65) | ✅ superior |
| Decrypt sandbox | WASM, CEK never crosses FFI | same invariant **+ full transcript binding + audit + expiry** | ✅ matched & extended |
| Node channel | HTTPS `rejectUnauthorized:false` | **channel authenticates the node** (attested KEM key, sealed framed frames) | ✅ superior |
| Key rotation | redeploy a constant | **live share rotation/reconfig, CEK never reassembles** | ✅ superior |
| Isolation | escapable iframe | **microVM/WASM/frame tiers, zero ambient authority** | ✅ superior |
| Content addressing | Helia/Kubo | byte-identical, **fail-closed integrity at every tree level** | ✅ matched |
| Creator UI / upload | mature (ela.city) | **gap** | 🔴 PC2 ahead |
| Non-media players | `wasm-renderer` (pdf/epub/cbz/image/code) | **gap** (no capsule yet) | 🔴 PC2 ahead |
| Live buy/trade + broadcast | live on Base | calldata done, **live broadcast/purchase gap** | 🔴 PC2 ahead |

**Architecturally ahead on the trust/crypto core; behind on product surfaces.**

---

## 4. Media vs non-media, Lit vs PQ

- **Media (today): ✅ playing.** `elacity-player` + the rail; real video decrypts in Home.
- **Non-media (docs/images/3D): 🔴 not yet a capsule.** Decrypt is content-agnostic so the
  *bytes* decrypt; the missing piece is a `ddrm-viewer` render-tier capsule (port PC2's
  `wasm-renderer`: pdf/epub/cbz/image/code, **plus a new 3D tier** PC2 lacks). **← active slice.**
- **Lit assets: 🟡 seam ready, adapter not shipped.** `key-provider` has an operator-selectable
  `lit` backend slot; it fails closed because no Lit proxy ships. Playing legacy Lit content =
  wire **one Lit-proxy adapter behind key-provider** — no app/decrypt change.
- **Quantum-resistant dKMS: 🟢 built, needs deployment.** The PQ threshold network runs as real
  daemons over TCP today (proven across 3+ nodes). "Node deployment" = standing those daemons up
  off-box in production (ops + provisioning), not new crypto.

---

## 5. Roadmap — sequenced thin vertical slices

1. **Non-media viewer** (`ddrm-viewer`: documents/images, then 3D) — biggest capability gap,
   reuses the proven rail. **← in progress.**
2. **Lift local adapters behind the unified Carrier capability plane** — close the §2 maturity
   gap so no capsule names a transport.
3. **Live chain** — wallet-sign → broadcast mint to Base + live `eth_getLogs`.
4. **Buy/trade** — the `buyAccess` purchase flow + UI.
5. **dKMS production** — multi-node off-box deployment + provisioning.
6. **Lit compat adapter** — serve legacy Lit-escrowed assets through the runtime.
7. **Creator app** — the package-and-publish surface tying it together.

Each slice: contract-first → characterization/golden tests → fail-closed proof → live
demoable increment → honest status + next-slice prompt (per `CONVERGENCE_PLAYBOOK.md` §8).

---

## 6. Principle conformance (non-negotiable)

- dDRM is the crown jewel; every slice serves the protected-content loop.
- Capability security: each step is a capability-scoped provider call; no ambient authority;
  the decrypt VM has no outbound key-fetch.
- Fail-closed by default; live wiring behind dev/feature profiles; shared contract
  byte-identical until blessed.
- CEK clear only inside the decrypt sandbox, in `Zeroizing`, bound to the full transcript;
  never on the wire, never to the app.
- ElastOS-native PQ-hybrid is the root; P-256/Lit is compatibility, not product truth.

---

## 7. Chain access (PC2 fidelity) — how ownership is queried

PC2 gates a viewer on **on-chain access-token ownership** (the predicate Lit's
access-control-conditions wrapped). The runtime reproduces that predicate natively via the
`chain-provider` capsule, so the answer is owned by us, not Lit.

- **Contract method:** `hasAccessByContentId(string contentId, address subject, string right)
  → bool` — the AuthorityGateway read. Encoded by `chain-provider/src/abi.rs::
  encode_has_access_by_content_id_call` and decoded by `decode_evm_bool` (exactly 32 bytes,
  high bytes zero, last byte 0/1). The selector is **supplied by config**, never computed
  in-capsule.
- **`contentId`:** the asset's on-chain content identity == the **bytes16 KID** the producer
  minted (see `abi.rs::abi_word_bytes16`, "`contentId == KID`", and `mint()` `opRawData`),
  surfaced as the `string` the read keys on. Must match byte-for-byte or the read fails
  closed. Local owned files (not real Elacity mints) use the object CID as the identifier;
  a real purchased asset uses its minted KID/contentId.
- **`right`:** `view` for render/playback (the same set chain-provider validates:
  view/stream/download/execute).
- **`subject`:** the buyer's EVM wallet address. In Home this is the signed-in principal's
  linked `eip155:` account (`wallet-provider` `accounts`), or `ELASTOS_DDRM_SUBJECT`.
- **Network/RPC:** Base (chain id 8453) via `ELASTOS_CHAIN_BASE_RPC`; the contract +
  selector are configured, never hard-coded into product paths.

**Where it runs.** The canonical CLI vertical (`scripts/dev/ddrm-runtime-open`) has driven
this real path since Day 136–140 with a `ChainRpcMock` for offline proof. As of
`feat/ddrm-home-playback`, the **Home gateway** open path (`/api/viewers/open`) drives the
SAME real `chain-provider` read behind the rights gate (`ELASTOS_DDRM_RIGHTS=chain` /
`chain-mock`), feeds the typed `ChainAccessAttestationV1` into `rights-provider.
decide_access_from_chain`, and welds the minted receipt hash into the decrypt transcript.
Owned → opens; not-owned → `403`, nothing sealed; no wallet in chain mode → fail closed.

**Still open:** the live *purchase* flow (buyAccess tx) that puts a token in the wallet in
the first place, and wiring a real Base RPC + the production Elacity contract/selector.

---

*Living document. Update as slices land and the live chain / dKMS deployment / Lit adapter
come online.*
