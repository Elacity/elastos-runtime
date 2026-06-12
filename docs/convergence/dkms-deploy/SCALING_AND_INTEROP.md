# dKMS Scaling, Node Interaction & ela.city Interoperability — Briefing

A team-shareable briefing answering: *can we go beyond 3 nodes? how do nodes actually talk? how
does this interoperate with the live ela.city portal? what are the limits, the risks, and the wins
before we integrate?* Everything below is grounded in the real code (`capsules/ddrm-envelope`,
`capsules/dkms-authority`, `capsules/key-provider`) — not aspiration.

---

## 1. Can there be 12 nodes? (scaling the quorum)

**The cryptography: yes — up to 255 nodes, at any threshold.** The split is Shamir over GF(256),
so node coordinates are bytes `1..=255` (x=0 *is* the secret). The general combine
(`lagrange_combine_at_zero`) reconstructs from any `t` points, and the reconfiguration primitive
(`reshare_eval` + combine) lifts a live set to a new `(k, m)` **without ever reassembling the CEK** —
there is a passing test that re-shares a **2-of-3 into a 3-of-5** preserving the same key.

**What ships TODAY is capped at `t=2`, `n ∈ {2,3}`** in two deliberate places:
- the publish-time split `split_cek_shamir2` deals **exactly 3 shares** at x=1,2,3 on a **degree-1**
  (t=2) polynomial; and
- the runtime descriptor parser accepts **EXACTLY `t==2` with 2 or 3 nodes** and fails closed on
  anything else (no silent degradation).

So **12 nodes is reachable, but not by editing a config** — it needs a small, well-scoped lift:
generalize `split_cek_shamir2` to `n` shares at an arbitrary `t`, and widen the parser's
`t==2 && n∈{2,3}` guard. The math, the node-set pinning (`threshold_node_set_id_n` over all vks),
the attestation, and the reconfiguration lifecycle are **already general**.

### The threshold trade-off you must pick consciously

| | More nodes at **fixed t=2** | Raise **t** with n |
|---|---|---|
| Availability | ↑ (survives more dead nodes) | ↓ relative to fixed t |
| Confidentiality | **↓** — any **2 of n** colluding recover the key | ↑ — needs `t` colluders |
| Right answer | almost never want 2-of-12 | e.g. **5-of-12**, **3-of-5** |

A "12 node" deployment should be something like **5-of-12**, not 2-of-12 — otherwise you've made
the key *easier* to steal (more independent parties any two of whom suffice) while only buying
availability. The reconfiguration primitive exists precisely so you can move `t` and `n` together,
live, without re-escrowing content.

### Performance & scaling notes

- **Opens don't get slower with more nodes.** An open contacts only `t` nodes (the fastest/available
  ones). `n=12, t=5` → an open talks to 5, not 12. Latency is dominated by the slowest of the `t`
  you reach, plus one PQ-channel handshake per node.
- **Ceremonies scale ~O(n²).** DKG and reconfiguration route a sealed sub-share for every
  dealer→target pair, so a 12-node ceremony is ~144 sealed blobs. Fine operationally, but it's a
  per-membership-change cost, not a per-open cost.
- **Operational surface scales linearly.** 12 nodes = 12 master seeds to back up, 12 hosts to patch,
  12 failure domains to monitor. The security win of independence only holds if they're genuinely
  independent (different providers/regions/operators).

**Recommendation:** launch **2-of-3** (shipped, verified). Grow to a larger `(t,n)` via the
reconfiguration lifecycle once it's promoted to the live path and you actually need more independence
or availability. Don't run a large `n` at `t=2`.

---

## 2. How do the nodes interact? (and where Carrier fits)

Two distinct conversations, **neither of which uses Carrier**:

**(a) Runtime ↔ node — the recover path (every open).** Direct, dedicated **framed transport**: the
descriptor pins each node's explicit `tcp:HOST:PORT` (or a unix socket), and `key-provider` connects
to it. Over TCP every post-handshake frame is a **sealed, mutually-authenticated, replay-bound**
envelope (the node attests a master-derived channel key under its pinned identity at `hello`). This
is a **separate transport from Carrier** — it addresses nodes by endpoint + pinned key, not by
Carrier DID/gossip.

**(b) Node ↔ node — only during a ceremony (DKG / reconfiguration).** There is **no direct
node-to-node socket** in the shipped code. Each sub-share is **sealed + signed, AEAD-bound to its
(dealer → target) pair**, and an operator/coordinator **routes the opaque blobs**. The courier is
untrusted by construction: it can't read a sub-share, can't redirect it to a different target, and a
tampered one is refused with the dealer named. So nodes "interact" through end-to-end-authenticated
blobs carried by the coordinator — not a peer mesh.

**What Carrier is (and isn't, here).** Carrier is the runtime's general decentralized comms substrate
(iroh gossip mesh today) for `elastos://` operations and capsule-to-capsule messaging. The dKMS rail
**does not ride Carrier** — it's a dedicated, hardened key-authority transport. A dKMS node is its
**own capsule** (`dkms-authority`), a long-lived daemon the runtime is a *client* of; it is not a
Carrier peer and it does not run inside the runtime process. Keeping them separate is deliberate: the
key authority gets its own minimal, auditable wire instead of inheriting the whole p2p surface.

---

## 3. Interoperability with the live ela.city portal

**What's shared (and already wired).** ela.city and the runtime sit on the **same chain (Base) and
the same contracts**: `AuthorityGateway` (`hasAccessByContentId`, `buyAccess`), the ERC-1155 access
token, USDC. The runtime's `chain-provider` now speaks those **real ABIs**. Consequence: **an asset a
user buys on ela.city is owned on-chain, and the runtime's rights check sees that same ownership** —
rights are interoperable by virtue of being on-chain truth, not a private database.

**Where they diverge — key delivery (the crux).** This is the one thing to internalize:

| | ela.city / base.ela.city (today) | ElastOS runtime (native) |
|---|---|---|
| Rights | Base contracts | **same** Base contracts |
| Key authority (CEK) | **Lit protocol** network | **dKMS PQ quorum** |
| Player | browser | runtime decrypt capsule |

Because the **key authority differs**, content is openable by whichever authority holds its CEK:
- **Content escrowed to Lit** (everything on ela.city today) is openable in the browser. The runtime
  can *also* open it **only via a Lit compat proxy** — that slot exists and **fails closed** because
  no proxy ships yet.
- **Content published natively** (CEK escrowed to your dKMS quorum) is openable by the runtime, but
  **ela.city's Lit-based player cannot open it** until a bridge exists.

This is the **"one fork"** from `STRATEGIC_ROADMAP.md`, and you've already chosen the north star:
**native PQ end-to-end**. So the clean scoping is: **new/native publishes target the dKMS quorum**;
legacy Lit content keeps using ela.city's path until migrated.

**What you can do today / your limitations:**
- ✅ Read the same on-chain rights as ela.city (buy on ela.city → runtime sees ownership).
- ✅ Mint + buy on-chain from the runtime (live `wallet_signer` → `chain-provider` rails).
- ✅ Open **native dKMS-escrowed** content end-to-end in the runtime (verified).
- ⛔ Open **Lit-escrowed** ela.city content in the runtime — needs the Lit proxy (fails closed now).
- ⛔ Play **native dKMS** content in the **ela.city browser** — needs a runtime-side player/bridge.

**Be mindful of:** don't assume "bought on ela.city" ⇒ "openable in runtime" — that's true for
*rights*, not yet for the *key* of Lit-escrowed titles. Pick, per content, which authority holds the
CEK, and don't strand a title between the two.

---

## 4. Pre-integration checklist (what to settle before node deployment)

1. **Threshold policy.** Confirm launch = **2-of-3** (recommended). Note 12-node / higher-t is a
   future reconfiguration step, not launch scope.
2. **Node independence.** Three genuinely independent failure domains (Interserver, Contabo, a third
   distinct provider/region). Independence is the entire security premise.
3. **Native-first scoping.** Confirm new publishes go to dKMS; Lit content stays on ela.city until a
   migration plan exists. (This is the STRATEGIC_ROADMAP fork — already chosen native.)
4. **Master-seed backup discipline.** Each node's `master.seed` backed up offline + encrypted;
   losing it strands every CEK escrowed to that node. This is the #1 operational risk.
5. **Operator key custody.** The operator signing key lives off the nodes; it authorizes every
   lifecycle op. Decide where it lives (a 4th box / HSM / your laptop) and who holds it.
6. **Migration question (deferred, but flag it now).** If you ever want ela.city titles openable in
   the runtime, that's a Lit-proxy or a re-mint/re-escrow migration — a separate project. Decide if
   it's on the roadmap or explicitly out of scope.

---

## 5. What to be excited about

- **You own the key authority.** `t`, `n`, the field arithmetic, the quorum policy, the failover, the
  refresh, and the attestation are all explicit and yours — versus renting Lit's opaque, immutable
  BLS network.
- **Post-quantum, today.** PQ-hybrid sealing on the live rail; the channel and escrows are PQ-authenticated.
- **Provable opens.** Every quorum release emits a portable `QuorumReleaseProofV1` an auditor verifies
  **offline from a file**, naming the serving node-set — something PC2/Lit cannot give you.
- **Born-distributed keys.** DKG means a CEK can exist *nowhere* even at birth (gates 49–51, verified).
- **Live, lossless reconfiguration.** Change membership and threshold without reassembling or
  re-escrowing keys (2-of-3 → 3-of-5 proven).
- **Rights interop is free.** Same Base contracts as ela.city — no parallel rights system to maintain.
