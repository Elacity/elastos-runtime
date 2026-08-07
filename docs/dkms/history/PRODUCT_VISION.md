# ElastOS — Product Vision & PRD

> Companion to `docs/dkms/history/CONVERGENCE_PLAYBOOK.md` (the how) — this is the
> *why* and the *what*: the macro picture and the finished-product definition any
> agent or contributor should share before working on convergence.
>
> Read alongside `PRINCIPLES.md` (the constitution). Separate **true-today** from
> **target-state** as noted.

---

## One-liner

**ElastOS** — a sovereign, capability-secured personal operating system (a small
trusted **Rust** core plus **isolated capsules/providers**), with the Elacity dDRM
economy built in.

> *The personal computer you actually own — your files, identity, money, AI agents,
> and a market for the content you create and consume, all under one
> capability-secured roof.*

---

## What it actually is (technical reality, grounded in the repo)

We are **not** transpiling Puter line-by-line into Rust. We are **re-platforming a
web OS onto a capability-secure Rust kernel**: keep the web UI, but move every
dangerous power behind isolated, capability-scoped Rust providers.

Three isolation tiers, strongest where the danger is:

| Layer | Written in | Runs as | Isolation |
|---|---|---|---|
| **Trusted core** (Runtime, ~10 crates) | Pure **Rust** | Native host process | It *is* the enforcement boundary; brokers all capabilities |
| **Providers** (`decrypt`, `key`, `rights`, `drm`, `chain`, `wallet`, `ai`, `ipfs`, `did`, `net`, `tunnel`…) | **Rust** | `type: microvm` → microVM (crosvm/Linux, Apple VZ/macOS) | **VM-level**: separate kernel + memory; no host FS/network unless granted. Used for the dangerous-authority plane (keys, crypto, chain). |
| **Shells / system logic** (`home`, `home-cli`, `system`/Settings, `chat-room`) | **Rust → `wasm32-wasip1`** | `type: wasm` → wasmtime | **WASM sandbox** (linear memory + WASI scope) |
| **App / content / UI** (`library`/Files, `marketplace`, `wallet` UIs, viewers, `inbox`, `browser`) | **Web (HTML/CSS/JS)** + signed manifest | `type: data` → runtime-mediated browser principal | **Frame-level**: zero ambient authority, capability-scoped |

**Every** capsule starts at zero authority and must *request* capabilities; none can
touch filesystem/network/keys/chain ambiently; all communication goes through the
**Carrier/provider capability plane** (`PRINCIPLES.md` #3, #4, #16).

**On PC2/Puter:** PC2 is Elacity's fork of Puter (Node.js/TypeScript), which already
contains some Rust→WASM crates (`cenc-decrypt`, `ddrm-*`, `mp4-split`,
`evm-multicall`…). Convergence = (a) re-found the trusted/authority plane in Rust,
(b) move PC2's Rust/WASM crates across as **provider internals**, (c) keep the web UI
but **de-privilege** it, (d) re-express PC2's product patterns as capsules against the
capability model. The "optimization" is primarily **trust, isolation, verifiability,
sovereignty, and portability** — performance/footprint are bonuses.

---

## Problem / status quo

Today your data, identity, and content live inside platforms that hold the keys and
the authority. "Web3" promised ownership but mostly delivers tokens, not a usable OS.
Puter proved the UX of a web desktop but inherits web-app trust assumptions (broad
sessions, ambient authority).

## Vision

Re-found the web OS on a small trusted Rust kernel where **authority is explicit and
isolated**, so users genuinely own their objects, encrypted content is normal, AI
agents are safely capability-bound, and creators can turn content into tradeable
capital — all verifiable.

## Personas

1. **Sovereign user** — owns files/identity/money.
2. **Creator / seller** — packages and monetizes content/apps with enforced rights.
3. **Developer** — ships capsules against a stable capability contract.
4. **Operator** — runs auditable, fail-closed infrastructure.
5. **AI agent** — a first-class, capability-scoped principal.

## Core value propositions

- **Ownership** — rooted identity (`localhost://`, `elastos://`) + signed content (DID/CID/hash/signature).
- **Security** — zero ambient authority; microVM/WASM/frame isolation; fail-closed.
- **Markets** — dDRM packaging, buy/trade, key-mediated decrypt (data → capital).
- **Safe AI** — one authority model for humans and agents.
- **Portability** — same capability model on Linux, Apple Silicon, and edge.

## Pillars (feature areas)

1. **Home & shell** — launch capsules; mint scoped launch capabilities.
2. **Files / Library** — objects in `localhost://`; viewers as capsules.
3. **System / Settings** — policy, recovery, storage, identity.
4. **Identity & Wallet** — passkey-first principals; wallets as proof bindings; external wallets as isolated connector capsules.
5. **dDRM economy** — package → rights → key → decrypt/render → marketplace. **The crown jewel.**
6. **AI plane** — agents as capability-scoped principals (`compute:ai-*`). *(design open)*
7. **Networking / Carrier** — local + off-box capability transport; content availability with signed receipts. *(design open)*

## The dDRM "data into capital" engine (crown jewel)

Fail-closed provider chain, each arrow a capability call:

```
drm/open → rights-provider → key-provider → decrypt-provider → scoped output
           (entitlement)     (CEK receipt)   (decrypt/render)    (never the CEK)
```

- The **CEK lives only inside the decrypt microVM**, is zeroized after use, and is
  never returned/logged/surfaced. Apps receive output scoped by `output_kind`.
- This lets creators **package** content/capsules, set rights/price, and let others
  **buy/trade** access while the platform never exposes keys — content and creativity
  become **owned, tradeable, programmable capital** with on-chain settlement.
- PC2 assets that map here as provider internals: `cenc-decrypt`, `ddrm-decrypt`,
  `ddrm-renderer`, `.ddrm` data capsules.

## Non-goals

- Not a line-by-line port of Puter.
- Not a general crypto wallet.
- Not exposing raw keys/RPC/IPFS/CEKs to app capsules.
- Not a flag-day rewrite (Strangler Fig, one boundary at a time).

## Success metrics

- Every sensitive operation is capability-scoped and audited (emits a receipt).
- Zero ambient-authority escapes.
- dDRM decrypt with provable CEK containment.
- Creators can package → sell → decrypt end-to-end.
- The same capability model runs cross-platform.

## Roadmap (and where current tasks sit)

- **Phase 0 (done):** authority surfaces declared (v0.3.0); CVE hygiene; Mac VZ substrate proven.
- **Phase 1 — Anders (now):** File Explorer + Settings + marketplace as capsules (v0.4).
- **Phase 1 — us (now):** **dDRM backend** — decrypt engine vendored behind the fail-closed contract (Day 1 done); next: wire `rights → key → decrypt → render` end-to-end with CEK containment.
- **Phase 2:** the **markets** — packaging, buy/trade, on-chain rights gating (chain/wallet providers).
- **Phase 3:** **AI plane** + **networking/Carrier** generation (the open questions).

## Risks

- Capability-contract drift (one already found + fixed in Day 1).
- Blockchain gap gates the market layer.
- AI/networking designs unsettled (intentionally — design later).
- Keeping the trusted core small as features grow.

---

## Industry framing (why this is hard and special)

- **OS/systems:** a capability OS (seL4/Genode/Fuchsia spirit), tiny trusted kernel + least-privilege capsules.
- **Security:** zero-trust/least-privilege taken to its end; fail-closed; auditable receipts; provable key containment.
- **Data sovereignty / Web3:** content-addressed identity; trust travels with signed content; own/encrypt/publish/sell without surrendering data.
- **Product:** Puter-grade desktop UX where every surface is a sandboxed capsule.
- **Economics:** dDRM + marketplace + chain/wallet = data and creativity as enforceable, tradeable capital.
- **AI:** humans and agents share one authority model — the only sane safety story for agentic computing.

---

*Living document. Update as contracts land, markets are wired, and the AI/networking
architecture is decided.*
