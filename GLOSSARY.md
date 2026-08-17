# ElastOS Glossary

A single-page orientation to the vocabulary of this repository — for developers,
engineers, and AI agents onboarding to the ElastOS runtime and the Elacity
platform built on it. Each entry gives a one-paragraph definition and, where one
exists, a pointer to the authoritative document.

Terms are grouped by layer, from the runtime core outward to the product rails.
A term in **bold** inside a definition has its own entry.

> Branch note: this `review/dkms-foundation` branch is the **substrate** — runtime,
> capsules, content protection (**dDRM**), key management (**dKMS**), chain, and
> marketplace. The **Flint** agent-governance layer (**mandates**, **intent-proof**,
> **spend meter**, the **Capsule Inspector** custody panel, and **ESP**) lives on the
> `review/flint` branch, which stacks on top of this one. Entries for those terms are
> included here so the vocabulary is complete, and marked _(Flint branch)_.

---

## Platform & runtime core

**ElastOS** — the decentralized operating substrate this repository implements: a
Rust runtime that launches sandboxed **capsules**, brokers their access to
**providers** through **capability tokens**, and records what happened on a
tamper-evident **audit chain**. It is the foundation the Elacity product runs on.
Docs: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md),
[docs/dkms/history/SYSTEM_ARCHITECTURE_MAP.md](docs/dkms/history/SYSTEM_ARCHITECTURE_MAP.md),
[docs/dkms/ARCHITECTURE.md](docs/dkms/ARCHITECTURE.md).

**Elacity** — the Web3 media + NFT product built on ElastOS: creator channels,
token-gated streaming, and an NFT **marketplace**, with blockchain-gated **dDRM**
protecting the content. Docs: [docs/dkms/history/PRODUCT_VISION.md](docs/dkms/history/PRODUCT_VISION.md).

**Runtime** — the `elastos-runtime` crate: the trusted core that owns the
**capability manager**, **provider registry**, session registry, and **audit
chain**, and mediates every capsule act. The `elastos-server` crate wraps it with
the HTTP/gateway surface and the **supervisor**.

**Supervisor** — the component that manages **microVM** capsule lifecycle (load,
start, network, stop) and wires each running capsule's **carrier bridge**.

**Gateway** — the HTTP surface that serves capsule UIs and brokers browser-hosted
capsules to the runtime. A _control-plane_ gateway serves infrastructure capsules
under service authority; the _serve_ gateway carries user act surfaces.

---

## Capsules & providers

**Capsule** — the unit of deployable, sandboxed code, described by a signed
`capsule.json` **manifest**. Three execution kinds: **microVM** (crosvm/VZ),
**Wasm** (in-process WASI sandbox), and **web/browser** (iframe UI). A capsule
never makes a raw syscall — all I/O crosses the **carrier bridge** as newline-
delimited JSON and is gated by **capability tokens**.

**Provider** — a capsule that exposes an `elastos://<scheme>/...` capability
surface consumed by other capsules or the host (e.g. `net-provider`,
`chain-provider`, `decrypt-provider`, `key-provider`). The `provider_resource`
map turns each provider operation into the **capability** action it requires
(fail-closed: an unmapped op requires `Admin`).

**Manifest / affordances / interfaces** — a capsule's `capsule.json` declares its
type, permissions, and `interfaces[]` — the typed **affordances** (methods with
risk, approval mode, and input schema) it exposes as tools. "By data, not code":
a capsule is a discoverable typed tool through its manifest.

**Carrier** — the transport plane. Two senses: (1) the **carrier bridge**, the
JSON-line request/response channel between a capsule and the runtime that mints
and validates the **capability token** for each act; (2) the **Carrier network**
(iroh: QUIC + pkarr DHT + hole-punching) used to reach **dKMS** quorum nodes
without a hand-provisioned VPN. Docs: [docs/CARRIER.md](docs/CARRIER.md),
[docs/DKMS_OVER_CARRIER.md](docs/DKMS_OVER_CARRIER.md).

**MicroVM** — a capsule isolated in a lightweight VM: `crosvm` over KVM on Linux,
Apple Virtualization.framework on macOS (native arm64 subprocesses in dev). The
per-TAP **egress firewall** default-drops guest network egress. Docs:
[docs/MICROVM_LOCAL_KVM_PROVISIONING.md](docs/MICROVM_LOCAL_KVM_PROVISIONING.md).

---

## Content protection — dDRM

**dDRM** — decentralized DRM: the rail that keeps media and non-media assets
encrypted end-to-end, releasing a content key only to an entitled viewer and
decrypting inside a sandbox so no plaintext key ever crosses to the player.
Docs: [docs/dkms/SECURITY_MODEL.md](docs/dkms/SECURITY_MODEL.md),
[docs/dkms/history/DDRM_STATUS.md](docs/dkms/history/DDRM_STATUS.md).

**CENC** — Common Encryption (ISO/IEC 23001-7): the AES-128-CTR scheme used to
encrypt media samples. The container-agnostic cipher core lives in the
`cenc-core` crate, shared by the encrypt and decrypt providers so the keystream
has one implementation. Docs: [docs/dkms/history/DDRM_ENCRYPT_INVARIANT.md](docs/dkms/history/DDRM_ENCRYPT_INVARIANT.md),
[docs/dkms/history/DDRM_DECRYPT_RAIL.md](docs/dkms/history/DDRM_DECRYPT_RAIL.md).

**CEK** — Content Encryption Key: the symmetric key that encrypts an asset's
bytes. Invariant #1: the CEK is minted **inside** the encrypt boundary and never
leaves in plaintext. Invariant #2: on open, the CEK is unwrapped and used to
decrypt in one colocated boundary, then zeroized — it never reaches the player.

**Envelope** — the `ddrm-envelope` crate: the single source of truth for the
`elastos-pq-hybrid-threshold-v0` seal — a post-quantum hybrid KEM (x25519 +
ML-KEM-768 + AES-256-GCM) that wraps the **CEK**, plus transcript-AAD binding and
the **Shamir**/threshold split used by **dKMS**.

**Pixel-lock / render-IR** — for non-media assets (PDF, images, comics, code,
SVG, EPUB), the decrypt boundary rasterizes/renders server-side and hands the
client only pixels or a render intermediate, never the source bytes — with
visible + invisible (DCT-QIM) **watermarking**. Asset tiers are defined in
[docs/ASSET_TIERS.md](docs/ASSET_TIERS.md).

**AV watermarking / forensics** — the (in-progress) forensic fingerprinting of
audio/video so a leaked copy is buyer-attributable (canonical codeword +
Tardos accusation). Today AV is key-protected, not yet fingerprinted. Docs:
[docs/AV_WATERMARKING.md](docs/AV_WATERMARKING.md); reference suite under
`tools/av-forensics/`.

**PSSH / KID** — CENC boxes and identifiers: the **KID** (16-byte key id) names an
asset's key on-chain and in metadata; the `pssh` box carries protection-system
data parsed/built by the envelope layer.

---

## Key management — dKMS

**dKMS** — decentralized Key Management System: the quorum of authority nodes that
custody the **CEK** shares and release a reconstructable key only to a caller who
proves on-chain entitlement. No single node holds the whole key. Docs:
[docs/DKMS_NODE_PROVISIONING.md](docs/DKMS_NODE_PROVISIONING.md),
[docs/dkms/deploy/](docs/dkms/deploy/).

**Shamir / threshold (t-of-n)** — the secret-sharing scheme (over GF(256)) that
splits a **CEK** into `n` shares so any `t` reconstruct it and `t−1` reveal
nothing. The wired product path is 2-of-3; general t-of-n and the pool-custody
design are specced in
[docs/superpowers/specs/2026-07-14-dkms-tofn-pool-custody-design.md](docs/superpowers/specs/2026-07-14-dkms-tofn-pool-custody-design.md).

**Quorum** — the specific set of **dKMS** nodes that hold an asset's shares. Each
node runs `dkms-authority`; a caller runs the recover protocol against ≥ t of
them, each using its own stored shard, and combines in the decrypt sandbox.

**DKG** — Distributed Key Generation: the ceremony by which nodes jointly derive
shares so the key is "born distributed" and existed as a whole nowhere. Docs:
[docs/dkms/deploy/DKG_CEREMONY.md](docs/dkms/deploy/DKG_CEREMONY.md).

**Access delegation / recover proof** — the two-tier authorization: a wallet
signs an `AccessDelegation` authorizing a session key over a scope, and each
recover carries a session-key-signed request with a single-use nonce. Nodes
re-check on-chain `hasAccessByContentId` themselves. Replay/nonce and node-set
binding are enforced per node.

**Confidential compute** — the (research) direction of running **dKMS** nodes
under hardware attestation. Docs: [docs/CONFIDENTIAL_COMPUTE.md](docs/CONFIDENTIAL_COMPUTE.md).

---

## Chain & commerce — marketplace

**chain-provider** — the sole RPC declarant: the only capsule that makes raw EVM
calls. It resolves a **KID** to the real on-chain ledger `tokenId`, reads
listings, and assembles unsigned transactions. Chains: Base mainnet (8453) for
production.

**rights-provider** — the live-chain rights gate: answers
`hasAccessByContentId(holder, kid)` so the **dDRM** open path and the marketplace
can decide entitlement.

**Marketplace** — the commerce rail: publish → index → discover → buy → acquire →
open. `publish-provider` assembles (never signs) the mint; `content-market`
reconstructs listings from the chain's own logs/calldata; `buy_authority`
enforces the buy-drift invariant; `object-provider` acquire pins a bought asset
into the Library. Docs: [docs/dkms/COMMERCE.md](docs/dkms/COMMERCE.md),
[docs/DRM_MARKETPLACE_RAIL.md](docs/DRM_MARKETPLACE_RAIL.md).

**Buy invariant** — the money-path safety rule: the signed `buyAccess` tx binds
`(seller, tokenId, price, payToken, quantity)` re-read live from chain, and
aborts if a second pre-broadcast read drifts. Docs:
[docs/dkms/COMMERCE.md](docs/dkms/COMMERCE.md).

**availability-provider** — pinning/replication of encrypted published bytes +
metadata (an IPFS-cluster-style policy) so an asset stays retrievable.

---

## Security & audit primitives

**Capability token** — an unforgeable, scoped, often single-use grant that
authorizes one capsule to invoke one provider action on one resource. Minted and
validated by the **capability manager**; the **carrier bridge** checks the token's
_required_ action against the operation (never the token against itself).

**Capability manager** — the runtime component that mints, validates, and revokes
**capability tokens** and owns the signing key that writes the **audit chain**.

**Audit chain** — the tamper-evident, ed25519-signed, hash-linked record of what
happened (grants, uses, content opens). It self-verifies and detects tail
truncation via a head-anchor sidecar.

**Egress firewall** — the kernel-level, per-TAP nftables default-drop chains that
contain a **microVM**'s network egress (W1B). Generic isolation the runtime
applies to every guest.

**Signature verifier / verified signer** — the trusted-key check that resolves a
capsule's honest signer (Some only on a real ed25519 trusted-key match), so the
**inspector** can report "verified" strictly behind a genuine check.

---

## Agent governance — Flint / KEEP _(Flint branch)_

**Flint / KEEP** — the consent-and-audit layer for AI agents that stacks on this
substrate: it _enforces_ what an agent may do and _records_ everything on the
**audit chain**. Its funded wedge is EU AI Act (Art 12/14) agent-containment.
_(Lives on `review/flint`.)_

**Mandate / standing grant** — a self-declared, revocable authority under which an
agent may run acts unsupervised. Issuing mints a real **capability token**;
revoking is the autonomy kill switch (the gate re-reads the grant each dispatch).
_(Flint branch.)_

**Intent-proof loop** — the signed, verifiable record that an agent's act matched
a declared intent within its mandate envelope, projected as a presence-aware
custody channel (absent ≠ clean). _(Flint branch.)_

**Spend meter** — the per-agent, per-capsule budget (`SpendMeter`) that debits a
fail-closed money/act cap on every metered act; an unprovisioned key has zero
budget. _(Flint branch.)_

**Capsule Inspector** — the read-only tool + custody panel that surfaces a
capsule's DID/trust/manifest, granted-vs-required capabilities, **audit chain**
attestation, and (on Flint) the intent-proof and spend channels. Its object
inspection is substrate at the 0.6-dev base; its custody-panel/preview evolution
travels with Flint. Docs: [docs/INSPECT_DDRM_MERGE_NOTES.md](docs/INSPECT_DDRM_MERGE_NOTES.md).

**ESP** — the ElastOS Shell/Svelte frontend that renders the shell surfaces
(home, capsule detail, the inspector custody panel) and the Flint consent / EU AI
Act views. _(Lives under `elastos/esp/` on the Flint branch.)_

---

_See also: [README.md](README.md) for setup, [ROADMAP.md](ROADMAP.md) for
direction, [AGENTS.md](AGENTS.md) for agent-contributor guidance, and
[docs/](docs/) for the full design corpus._
