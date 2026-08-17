# Decentralized t-of-n dKMS Custody — Pool-DID Directory + One-Shard-Per-Node

Date: 2026-07-14 (updated 2026-07-15: scoped access-delegation v2 folded in — §6; updated
2026-07-16: custody trade-off discussion concluded and recorded — §14)
Status: **DRAFT — custody model deliberately OPEN.** The stateless vs stateful decision is
recorded as pros/cons for both (§14.2–14.3), the discussion outcomes (§14.4), an **invariant
improvement path that ships under either outcome** (§14.5), the two divergent paths
(§14.6–14.7), and the deciding questions for the ADR (§14.8). The §5 custody protocol below is
path B's machinery — read it as conditional. The crypto core (§4), scoped delegation (§6), and
recover-proof hardening (§8) ship under either outcome.
Sub-project 1 of the dKMS improvement program (fuses the original Phase-2 protocol
bump with the Phase-3 DHT custody work; the process-manager sub-project that preceded it is
postponed). Context: `docs/superpower/specs/dkms-improvements.md`.

## 0. Where this sits

Program ordering: (0) bugs — P0 CEK-commitment, P1 IPFS availability (now specced as
content-plane adoption: `2026-07-15-media-content-plane-adoption-design.md`), and the
standalone caller-seed-in-env fix; **(1) THIS SPEC**; (later) the canonical Runtime
stream/session contract, then external interop as its first use case (see the reshaped
`2026-07-14-io-interop-kit-design.md`). The process manager
was step 1 but is **postponed until ESP/System lands** (see
`2026-07-14-process-manager-design.md` — overlap with the existing Runtime supervisor);
nothing in this spec depends on it.

This spec is a **single coordinated breaking wire bump** delivered as one geo-node redeploy.
It deliberately fuses originally-separate phases (dynamic t-of-n + the DHT custody model)
because they share the same redeploy and the same audit surface. **Scoped access delegation v2
is IN** (§6): the 2026-07-15 re-evaluation established the per-asset kid binding is a
consent/blast-radius control, not the rights gate (each node's own on-chain check is), so a
**set-scoped** wallet signature ships in this same bump — one prompt can cover a channel or
playlist with zero security delta. The wallet-wide ("anything I own") scope stays OUT (§13).
The P0 CEK-commitment fix is **folded in** because the commitment becomes the standing integrity
backstop for the general scheme.

## 1. Goal

Replace the fixed, hardcoded 2-of-3 escrow (shares carried in the asset metadata, stateless
nodes) with a **decentralized, dynamically-sized t-of-n secret store**:

- The runtime picks `t` and `n` **in-code** at mint from the pool's available size (producer does
  not choose), capped and floored by config.
- The metadata no longer carries member keys or shares — only trusted **pins**. A **pool**,
  identified by a DID, is the directory of eligible nodes; a per-KID **DHT manifest** names the
  specific quorum; each **node holds exactly one sealed shard** for that KID.
- General t-of-n crypto (any threshold up to a cap), with cheater detection and golden vectors.
- **Scoped access delegation v2**: one wallet signature authorizes the session key over a
  user-visible **set** of KIDs (Merkle-rooted), bound to the durable `pool_did` instead of a
  per-asset node-set — one prompt per channel/binge session instead of one per asset (§6).
- The staged re-seal-AAD hardening ships in the same bump.

All existing security invariants hold: no raw CEK ever leaves the decrypt sandbox; every failure
fails closed; `seal-rail = open-rail`; the per-node imposter gate is unchanged.

## 2. Architecture — three authenticated layers

The metadata shrinks to *pins*; a directory in the DHT says *who*; the nodes hold the *what*.

1. **Pool** — a governed set of eligible nodes, identified by **`pool_did`** (a pkarr/DHT-published
   DID Document — the same pkarr DHT the Carrier layer already uses for node `did:key` discovery).
   Resolving it returns a **governance-signed** doc: member node DIDs, `threshold_cap`, `min_nodes`,
   `max_n`, and the governance key. *Responsibility: who may ever hold a shard.*
2. **KID-manifest** — a per-asset DHT record, keyed by KID, **producer-signed**: `pool_did`,
   `node_set_id`, `t`, `n`, the specific member node DIDs this asset was sealed to, `producer_vk`,
   `cek_commitment`. Public, small, no share material. *Responsibility: which specific nodes hold
   this KID's shards.*
3. **Node shard-store** — each node durably holds **exactly one sealed shard per KID** (sealed to
   itself; only its master seed opens it). *Responsibility: custody of one point on the polynomial.*
4. **Slim metadata** — on IPFS / the token, only the trusted **pins**: `kid`, `pool_did`,
   `node_set_id`, `cek_commitment`, `scheme`, `chain`. *Responsibility: the minimal anchor the
   client trusts.*

### Resolution chain at open (every hop authenticated)

```text
KID → metadata { pool_did, node_set_id, cek_commitment }          [pins: trusted, on-chain-linked]
    → resolve pool_did (pkarr) → pool doc, verify governance sig   [who may hold shards]
    → fetch KID-manifest (DHT), verify producer sig                [which nodes hold this KID]
    → keep only members whose vks reproduce node_set_id            [ANTI-SWAP PIN — the crux]
    → imposter-gated PQ recover against ≥t of them,
      each node using ITS OWN stored shard                         [per-node attestation]
    → nodes re-seal to one-time session key → combine in sandbox   [no-raw-CEK, unchanged]
```

Five independent gates matching the five things that could go wrong: a poisoned manifest fails
the `node_set_id` reproduction; an unauthorized member fails the governance signature; a
substituted node fails its challenge-attestation; an out-of-scope kid fails the Merkle membership
check against the wallet-signed delegation (§6); a wrong CEK fails the commitment.

**Reused (already n/t-generic):** the recover crypto (imposter gate, unwrap, re-seal, combine),
`threshold_node_set_id_n(t, vks)`, `cek_commitment(node_set_id, cek)`. **New:** the pool-DID
resolver, the KID-manifest, the per-node shard-store + mint-push + recover-by-KID protocol,
general degree-(t−1) checked reconstruction, and the auto-quorum policy.

## 3. Auto-quorum policy (t derived in-code)

At mint the runtime reads the pool's currently-eligible, reachable members `N`, then:

- **Fail closed if `N < min_nodes`** — no mint (default `min_nodes = 3`).
- **`n`** = all eligible available members, bounded by `max_n` (default 9). When `N > max_n`, the
  `n` members are chosen by a **stable, reproducible rule** — default: sort candidate DIDs
  lexicographically and take the first `max_n` (deterministic, not gameable by the producer, and
  the exact chosen set is recorded in the KID-manifest regardless). A latency/health-aware
  selection is a permitted future refinement behind the same recorded-in-manifest contract.
- **`t = min(threshold_cap, ⌊n/2⌋ + 1)`** — a strict majority, capped (default `threshold_cap = 5`).

| N available | n | ⌊n/2⌋+1 | t (cap 5) | quorum |
|---|---|---|---|---|
| 2 | — | — | — | **mint refused** |
| 3 | 3 | 2 | 2 | 2-of-3 |
| 5 | 5 | 3 | 3 | 3-of-5 |
| 7 | 7 | 4 | 4 | 4-of-7 |
| 9 | 9 | 5 | 5 | 5-of-9 |
| 15 | 9 | 5 | 5 | 5-of-9 |

`min_nodes`, `threshold_cap`, `max_n` live in config, read at mint. `t`, `n`, and the chosen
member set are frozen into the KID-manifest and `node_set_id` for that asset's life.

## 4. General t-of-n crypto

- **Split** — `split_cek_shamir(cek, coeffs, n)` (exists) with `t−1` random coefficients from the
  CSPRNG (kept outside the envelope crate, per existing policy) → degree-(t−1) polynomial, `n` shares.
- **Combine** — `lagrange_combine_at_zero` (exists) reconstructs from any `t` indexed shares.
- **Cheater detection at general t (NEW `combine_cek_checked`)**, generalizing the degree-1-only
  `combine_cek_shamir2_checked`:
  - `m > t` served: reconstruct from one `t`-subset, then verify the remaining `m−t` shares lie on
    the reconstructed degree-(t−1) polynomial (evaluate at their x, constant-time compare). A single
    wrong-valued share is off the curve → detected → fail closed. `O(m)` after one reconstruction.
  - `m == t` served: no cross-check possible — the **`cek_commitment` is the integrity backstop**
    (reconstruct → re-derive `SHA-256(domain ‖ node_set_id ‖ CEK)` → constant-time compare → fail
    closed on mismatch). This is why the **P0 commitment fix is folded in**.
- **Golden vectors (NEW)** — RNG-free deterministic vectors for split+combine at t∈{2,3,4,5}, shared
  by client and node, so they cannot drift.

Invariant: any `t−1` shares are information-theoretically nothing; a wrong share never yields a
silently-wrong CEK; the CEK is reconstructed only inside the decrypt sandbox.

### The CEK commitment, precisely

`cek_commitment_b64 = base64(SHA-256(domain ‖ node_set_id ‖ CEK))` (`ddrm_envelope::cek_commitment`).
A one-way, quorum-bound fingerprint of the CEK — not a key, not a share, reveals nothing
(pre-image resistant), verifiable only by a party that already reconstructed the exact CEK. The
decrypt boundary checks it BEFORE any content decryption and fails closed on mismatch. It is the
ONLY integrity guarantee available when exactly `t` shares serve (no cross-check possible), which
in a general t-of-n world is a normal operating point at every threshold. It stays a **client-side
pin** (slim metadata + DHT manifest) and is **never sent to a node** — its only job is to let the
decrypt boundary catch a lying node.

## 5. Custody protocol (mint-push + recover-by-KID)

### Mint → push (the write path, NEW)

After split + seal (`share_i` sealed to `node_i`), the runtime **pushes each sealed shard to its
node**, which stores it keyed by KID, then **publishes the producer-signed KID-manifest** to the
DHT. Two write-path defenses:

- **Write-once per `(node, KID)`** — a node refuses to overwrite an existing shard. The KID is
  derived from the *secret* CEK (unpredictable/unforgeable), so first-write-wins is safe: an
  attacker can neither corrupt a live asset's shard nor pre-claim a KID it can't predict.
- **Allow-list-gated store** — a node accepts a shard-store only from a caller on its allow-list
  (the same identity gate that guards recover), plus a per-caller rate/quota to bound
  storage-fill DoS. The stored record is producer-signed so the node confirms the shard + context
  were authored by the same producer that signed the escrow seal.

### Node shard-store (NEW state)

Each node durably holds `KID → sealed_shard`, stored in the exact sealed form it was minted in
(only that node's master seed opens it). **Backup obligation:** shards are random, so a node that
loses its store loses those shards (tolerable up to `n−t`; backed up like the master seed already
is). Per-node record ⊇ today's `protections.shares[i]` slice plus the asset-level context the node
needs to recover locally:

```json
{ "kid": "0x…", "scheme": "elastos-pq-hybrid-threshold-v1", "node_set_id_b64": "…",
  "producer_verifying_key_b64": "…", "wrapped_share_b64": "…",  // x sealed inside
  "stored_at": 0, "producer_sig_b64": "…" }
```

### Open → recover-by-KID (protocol change)

The recover request no longer carries the share — it **names the KID**. The node looks up its own
stored shard, verifies its sealed AAD binds that exact KID, then runs the **unchanged** audited
path: imposter gate → unwrap with recipient secret → re-seal to the one-time session key → return.
No shard for the KID → fail closed. The re-seal-AAD hardening (§8) binds into the possession proof
here.

### Consequence to accept (recorded, not hidden)

Today's headline property — **"mint is 100% local, zero node contact"** — is given up: mint now
contacts `n` nodes to push shards. Mitigations: the push is a cheap **write** (no rights check, no
crypto ceremony), **async + idempotent-retryable** (write-once makes retries safe), and can be
fire-and-forget with confirmation so mint UX isn't blocked. This is the real price of "each node
holds its own shard."

## 6. Scoped access delegation v2 (one signature, a set of assets)

### Why the per-asset binding can be relaxed safely

Today's `AccessDelegationV1` (`elastos.ddrm.access.v1`, `ddrm-envelope/src/access.rs`) binds the
wallet signature to ONE `kid_hex` — one wallet prompt per asset per window. The 2026-07-15
re-evaluation established what that pin actually defends: it is **not the rights gate** — every
node unconditionally runs its own `hasAccessByContentId(holder, kid)` for the *requested* kid,
regardless of what the delegation says. The kid pin buys (a) **consent scope** (the prompt names
exactly what it authorizes) and (b) **blast radius** (a stolen session key opens one kid, not the
wallet's library). Both survive intact when the wallet signs a *set* instead of a single kid,
because the wallet still signs exactly what it authorizes. Two things do NOT survive
automatically — the per-asset watermark-anchor uniqueness and the node-set binding — and both are
handled explicitly below.

### The v2 delegation (wallet-signed)

`DELEGATION_DOMAIN` bumps `elastos.ddrm.access.v1` → `…/v2`. Two changes to the signed object:

1. **`scope` replaces `kid_hex`** — one of:
   - `{ "kind": "kid", "kid_hex": "0x…" }` — exact v1 semantics (default single-asset case).
   - `{ "kind": "kid-set", "merkle_root_b64": "…", "set_size": N }` — a SHA-256 Merkle root over
     the authorized kid set. Leaves are `SHA-256(0x00 ‖ kid16)` over the sorted, deduplicated
     16-byte kids; internal nodes are `SHA-256(0x01 ‖ left ‖ right)` (domain-separated against
     second-preimage games). `set_size` is displayed at prompt time and bounds proof depth.
2. **`pool_did` replaces `node_set_id_b64`** — in the pool world the node-set is chosen *per
   asset* (§3), so keeping a node-set binding would re-bind the delegation to one asset through
   the back door. The delegation binds the durable trust root (the pool); the **request keeps**
   the concrete `node_set_id_b64`, so anti-cross-quorum replay is preserved per open.

Everything else is unchanged: `chain_id`, `owner_address`, `covered_addresses`,
`session_pub_b64`, `issued_at`/`expires_at` (24h cap), `nonce_b64` (the revocation handle). The
session key is expected to be **reused across opens within the window** — that is the point; it
only authorizes, it never guards CEK material.

### The v2 request (session-key-signed)

`REQUEST_DOMAIN` bumps to `…request.v2`. Unchanged fields (`kid_hex`, `node_set_id_b64`, 60 s
freshness, single-use nonce) **plus, for kid-set scope, `scope_proof_b64`**: the Merkle path from
this kid's leaf to the signed root. The node verifies membership BEFORE any signature math and
fails closed (`BadScope`) on a missing/invalid proof or a kid outside the set.

### Node verification order (all fail-closed)

domain/structure → **scope membership** (kid ∈ signed set) → pool binding (delegation `pool_did`
== the node's configured pool pin, `DKMS_AUTHORITY_POOL_DID`, successor of today's node-set pin;
request `node_set_id_b64` must match the stored shard's sealed context) → replay/revocation
(unchanged `ReplayGuard`; same `(delegation_nonce, request_nonce)` single-use rule) →
EIP-191/EIP-1271 wallet signature → session-key request signature → **on-chain
`hasAccessByContentId` per covered address — unchanged and unconditional**.

**Invariant (new, load-bearing):** a delegation names *who may exercise* the wallet's existing
on-chain rights; it never grants rights. The per-kid chain check is the rights gate and runs on
every recover regardless of scope kind.

### Watermark anchor v2

`grant_watermark_digest16` currently hashes the delegation signature alone — per-asset anchor
uniqueness was a free side effect of per-asset signatures. For v2 grants the anchor becomes
`SHA-256(normalized_delegation_sig ‖ kid16)[..16]`, so every asset opened under one delegation
still carries a distinct, owner-attributable mark. Embedder (`ddrm-media-authority`) and
extractor (`decrypt-provider --extract-watermark`) dispatch on grant version; both formulas live
in `ddrm-envelope` (single source, anti-drift), and the v1 formula stays for v1 grants.

### Rail selection & compatibility

The access-grant version rides the asset scheme, same doctrine as everything else in this bump:

| Asset scheme | Grant | Binding |
|---|---|---|
| `…-threshold-v0` (legacy) | `access.v1` — verbatim: single kid + `node_set_id` | unchanged |
| `…-threshold-v1` (new) | `access.v2` — scope + `pool_did` | this section |

The new node binary verifies both domains; a v1 grant on the v1 rail (or a v2 grant on the v0
rail) fails closed. The gateway builds v2 grants only for v1 assets. Breaking node-side change →
ships in this same coordinated redeploy (the protocol-compat invariant forbids client-first).

### UX contract

The canonical JSON the wallet signs shows `kind`, the root, and `set_size`; the surface
presenting the prompt (portal/player) must name the set in human terms ("Channel X — 42 items").
A kid-set delegation is built from an **enumerable catalog known at prompt time** (channel,
playlist, library snapshot) — it is NOT "anything I own"; that is Tier B, deferred (§13).

### PC2 parity

`access.rs` deliberately mirrors PC2's `secureViewSession.ts` (byte-identical canonical encoder).
v2 diverges; the divergence is versioned by the domain tag, so nothing can drift silently.
Mirroring scope support back into PC2 is a coordination item (§13) — until then PC2 interop stays
on `access.v1`/single-kid.

## 7. Field mapping (v0 → v1)

Today's `asset.protections[0]`:
`{ algorithm, protectionType, scheme, chain, node_set_id_b64, producer_verifying_key_b64,
cek_commitment_b64, shares:[{verifying_key_b64, wrapped_share_b64, x}, …] }`

| v0 field | v1 home |
|---|---|
| `shares[i].wrapped_share_b64` (+ x inside) | node i's shard-store (only node i's entry) |
| `scheme`, `kid`, `node_set_id_b64`, `producer_verifying_key_b64` | node's stored record **and** DHT manifest (intentional redundancy — each side verifies its own copy; they must agree) |
| `cek_commitment_b64` | slim metadata pin + DHT manifest — **never to a node** |
| other nodes' `shares[j]`, `verifying_key_b64` | nowhere aggregated; members live as **DIDs** in the DHT manifest |
| `algorithm`, `protectionType`, `chain` | DHT manifest and/or slim metadata |

## 8. Schemas + the coordinated bump

**Slim metadata (IPFS / token):**
```json
{ "schema": "elastos.ddrm.asset/v1", "kid": "0x…", "chain": "base",
  "scheme": "elastos-pq-hybrid-threshold-v1", "protectionType": "cenc:elastos-pq-hybrid-threshold-v1",
  "pool_did": "did:dht:…", "node_set_id_b64": "…", "cek_commitment_b64": "…" }
```

**Pool DID document (pkarr, governance-signed):**
```json
{ "schema": "elastos.dkms.pool/v1", "pool_did": "did:dht:…",
  "members": ["did:key:z6Mk…", …], "policy": { "threshold_cap": 5, "min_nodes": 3, "max_n": 9 },
  "governance": { "scheme": "single-key-v1", "key_b64": "…" },
  "sig_b64": "…", "updated_at": 0 }
```
`governance.scheme` is shaped so `single-key-v1` can become a multisig or on-chain-DAO scheme
later without a further breaking bump. The governance key is the single new trust root (§9).

**KID-manifest (DHT, producer-signed):**
```json
{ "schema": "elastos.dkms.kid-manifest/v1", "kid": "0x…", "pool_did": "did:dht:…",
  "node_set_id_b64": "…", "t": 3, "n": 5, "members": ["did:key:z6Mk…", …],
  "producer_verifying_key_b64": "…", "cek_commitment_b64": "…", "sig_b64": "…" }
```

**Access delegation v2 (wallet-signed, EIP-191 over canonical JSON — §6):**
```json
{ "domain": "elastos.ddrm.access.v2", "chain_id": 8453,
  "scope": { "kind": "kid-set", "merkle_root_b64": "…", "set_size": 42 },
  "pool_did": "did:dht:…", "owner_address": "0x…", "covered_addresses": ["0x…"],
  "session_pub_b64": "…", "issued_at": 0, "expires_at": 0, "nonce_b64": "…" }
```

**Access request v2 (session-key-signed; `scope_proof_b64` present only for kid-set scope):**
```json
{ "domain": "elastos.ddrm.access.request.v2", "kid_hex": "0x…", "node_set_id_b64": "…",
  "requested_at": 0, "request_nonce_b64": "…", "scope_proof_b64": "…" }
```

**Recover-proof v2 (bundled hardening):** `DKMS_RECOVER_DOMAIN` bumps `/recover-proof/v1` →
`/v2`, and `recover_proof_message(...)` gains one bound field: **`sha256(reseal_aad)`**. Breaking
proof change → ships in this redeploy (nodes verify v2, client emits v2). Works for both v0 and v1
assets (binds recover context, not asset format).

**One geo-node redeploy delivers** a node binary that: verifies v2 recover proofs; verifies
access.v2 grants (scope membership + pool binding) alongside access.v1; supports
recover-by-KID (shard lookup) AND the legacy share-in-request path; accepts allow-listed write-once
shard-store; and does general t-of-n verification. The client, same release, emits v2 proofs,
builds access.v2 grants for v1 assets, resolves via pool-DID, and drives whichever rail the
asset's `scheme` selects.

### Migration — the scheme string is the rail selector

- **`elastos-pq-hybrid-threshold-v0`** (unchanged) → legacy rail: full `protections` in metadata,
  stateless share-in-request recover, fixed 2-of-3, **access.v1 grants** (single kid +
  `node_set_id` binding, v1 watermark anchor). Existing assets keep this scheme and keep
  opening untouched — the new node binary still speaks the v0 path.
- **`elastos-pq-hybrid-threshold-v1`** (new) → new rail: slim metadata + pool-DID + KID-manifest +
  node-held shards, general t-of-n, v2 recover-proof, **access.v2 grants** (scoped delegation +
  `pool_did` binding, kid-mixed watermark anchor — §6). Mint only ever writes v1.

`SUPPORTED_SCHEMES` becomes `["…-v0", "…-v1"]`; read/playback (`key-provider`, `decrypt-provider`,
`ddrm-media-authority`) dispatches on `scheme` and supports both indefinitely. `seal-rail =
open-rail` reads literally off the scheme: a v0 asset opens only on the v0 rail, a v1 asset only on
the v1 rail; a mismatch fails closed. No forced migration; an optional v0→v1 backfill is out of
scope and unnecessary for correctness.

## 9. Security model

| Threat | Gate | Result |
|---|---|---|
| Poisoned DHT manifest points at attacker nodes | member vks must reproduce the metadata `node_set_id` pin | fail closed (anti-swap) |
| Unauthorized node joins the pool to harvest shards | pool-doc membership change requires the governance signature | rejected at resolution |
| Impostor node answers a recover | per-node challenge-attestation (pin → challenge → channel binding) | fail closed at hello |
| Byzantine node returns a wrong-valued share | cross-check (m>t) or `cek_commitment` (m==t) | fail closed, never wrong-key decrypt |
| Attacker plants/overwrites a shard | write-once per (node,KID) + allow-list + producer-signed record | rejected at store |
| `t−1` colluding nodes | Shamir threshold | information-theoretically nothing |
| Relay/transport reads a recover | end-to-end PQ channel (bridges see ciphertext) | unchanged |
| Recover for a kid outside the wallet-signed set | Merkle membership proof against the signed root (§6) | fail closed (`BadScope`) |
| Stolen session key + set-scoped delegation | blast radius = the signed set for the remaining window; nonce revocation; on-chain rights gate still per kid | bounded — never "whole wallet" |
| v2 delegation replayed against another pool / quorum | `pool_did` binding in the delegation + `node_set_id` in each request | fail closed |
| Watermark anchor collision across assets under one delegation | v2 anchor mixes the kid: `SHA-256(sig ‖ kid16)[..16]` | per-asset unique, owner-attributable |

**Trust root:** the pool governance key is the single new trust root — the thing that decides who
may ever hold a shard. It is the most security-sensitive artifact in this phase. `single-key-v1`
now (operator-provisioned reality), schema-shaped for multisig/DAO later.

**Invariants held verbatim:** no raw CEK leaves the decrypt sandbox; every failure fails closed;
`seal-rail = open-rail` (scheme-keyed); the CEK commitment gates every reconstruction; **the
on-chain per-kid rights check is unconditional — a delegation names who may exercise rights, it
never grants them** (§6).

## 10. Error handling (all fail-closed)

- `N < min_nodes` at mint → refuse to mint (typed error, no partial escrow).
- Pool-DID unresolvable / governance sig invalid → refuse open.
- KID-manifest missing / producer sig invalid / members don't reproduce `node_set_id` → refuse open.
- Fewer than `t` shards recoverable → refuse, name the shortfall (`t of n required, k served`).
- Node has no shard for a v1 KID → "no shard held," fail closed.
- Commitment mismatch after reconstruction → refuse, never decrypt.
- Kid-set grant with a missing/invalid Merkle proof, or a kid outside the signed set →
  `BadScope`, refuse (checked before any signature math).
- Grant version ≠ asset rail (access.v1 grant on the v1 rail, or v2 on v0) → refuse.
- Delegation `pool_did` ≠ the node's configured pool pin → refuse.
- Shard-store push failure → mint retries (idempotent, write-once). **Default: mint requires all `n`
  pushes to confirm within a bound, else fail the mint** (durability-first; a knob could relax this
  to `≥ t + margin` later).

## 11. Testing

- **Golden vectors** for general split/combine at t∈{2,3,4,5} (RNG-free, client+node shared).
- **Crypto unit:** cross-check catches a Byzantine share at m>t; commitment catches it at m==t;
  `t−1` shares reveal nothing.
- **Policy:** the auto-`t` table (N=2 refused … N=15→5-of-9); cap and floor honored.
- **Resolution:** poisoned manifest rejected by `node_set_id`; unauthorized pool member rejected by
  governance sig; the full KID→open chain against fakes.
- **Custody:** write-once refuses overwrite; store refuses off-allow-list; recover-by-KID reads the
  stored shard; missing shard fails closed.
- **Scoped delegation:** Merkle golden vectors (set sizes {1, 2, 3, 5}, odd/even trees, duplicate
  kids deduped — RNG-free, client+node shared); membership proof verifies; a kid outside the set
  fails `BadScope`; a truncated/reordered proof fails; single-kid scope behaves exactly like v1;
  same `(delegation_nonce, request_nonce)` replay is rejected even across different kids.
- **Grant version dispatch:** access.v1 on the v0 rail opens; access.v1 on the v1 rail refused;
  access.v2 on the v0 rail refused; wrong `pool_did` refused.
- **Watermark anchor v2:** distinct kids under one delegation yield distinct anchors; the
  extractor recomputes the identical 16 bytes from the retained grant + kid.
- **Migration:** a v0 asset opens on the legacy rail unchanged; a v1 asset on the new rail; neither
  can be coerced onto the other (scheme mismatch fails closed).
- **Real-stack smoke:** extend the producer-carrier harness to mint v1 at 3-of-5, drop one node
  mid-recover (opens), drop two (fails closed) — the same shape as the existing 2-of-3 smoke; the
  smoke's opens run under ONE kid-set delegation covering all minted test assets.

## 12. Component boundaries (SOLID; sized for slicing the plan)

- `ddrm-envelope` — general split/`combine_cek_checked` + golden vectors (pure crypto); access v2
  (scope object, kid-set Merkle build/prove/verify, `pool_did` binding, watermark anchor v2 —
  shared client+node, anti-drift).
- pool resolver — `pool_did` (pkarr) → verified pool doc; a port (trait we own + default
  adapter) so the DID method / DHT can be swapped without touching the core.
- KID-manifest store/fetch — DHT read/write + producer-sig verify; a port.
- node shard-store — durable `KID → sealed_shard`, write-once, allow-listed, producer-sig verify.
- `dkms-authority` — recover-by-KID + legacy share-in-request; general t-of-n verify; v2 proof;
  verifies access v1 AND v2 (scope membership checked before signature math); pool pin env.
- `encrypt-provider` — auto-quorum policy + general split; unhardcode `!= 3`; publish commitment.
- gateway mint/open spine — scheme dispatch (v0/v1), slim metadata, orchestration; builds v2
  grants (set enumeration at prompt time, per-open scope proofs), caches the session key +
  delegation for the window.

Each has one responsibility and a testable interface; the plan will slice along these seams.

## 13. Out of scope (explicit)

- **Resharing** (changing t/n/members of an already-sealed asset via a DKG/proactive-refresh
  ceremony) — the `reshare_seed`/`dkg_seed` machinery stays unwired; its own future spec.
- **Tier B wallet-wide delegation scope** (`kind: "wallet"` — "anything I hold rights to") —
  deferred. Security-equivalent only with compensations that are real new machinery: a much
  shorter max window (minutes, not 24h), node-side per-delegation distinct-kid + velocity caps
  (attached to the `ReplayGuard` nonce maps — enforced independently per node, so it stays
  threshold-honest), and a user-driven revocation path (the `revoke` primitive exists; nothing
  drives it today). Recorded here so it lands as a scoped follow-up, not a redesign. Tier A
  (kid-set scope, §6) already covers the channel/playlist/binge UX.
- **PC2 mirror of access v2** — PC2 (`secureViewSession.ts`) stays on v1/single-kid until scope
  support is mirrored; the domain tag versions the divergence.
- **On-chain DAO governance** of the pool — schema is shaped for it; implementation deferred.
- **v0→v1 asset backfill** — unnecessary for correctness.
- Stake/permissionless pool joining — governance is a signed key for now.

## 14. Custody architecture — OPEN DECISION: stateless vs stateful
### (all-party record; updated 2026-07-16 after the step-by-step trade-off discussion)

**Deliberately left open.** Neither model is selected here. This section records both models'
pros/cons, the discussion outcomes that inform the choice, the improvement path that is
INVARIANT under either outcome, and the two divergent paths — so the eventual ADR is a
selection, not a re-investigation.

### 14.1 The two models, one line each

- **Stateless (today's shape, generalized):** sealed shares travel inside the asset's public
  metadata; nodes store nothing per-asset; at open the client brings each node its own share.
- **Stateful (this spec's §5):** each node durably holds its shard keyed by KID; metadata slims
  to pins; a DHT manifest names the holders; at open the client names the KID.

**Agreed by all parties:** collusion security is identical (t node compromises either way) —
the choice is about availability, operations, and product properties, not secrecy. Old assets
stay on the v0 rail untouched under either outcome.

### 14.2 Stateless — pros / cons

**Pros**
- Shares live on the public/cacheable plane (CTO caching doctrine: "encrypted published
  bytes"), made durable by the content-plane spec (pin-at-mint, receipts, repair fleet). Share
  availability = asset availability; no new dependency — metadata retrievability is already a
  hard prerequisite for playback in both models (no metadata ⇒ no ciphertext ⇒ nothing to open).
- Per-node survival requirement is one 32-byte seed, backed up once. Seed restored ⇒ every
  share ever sealed to that node works again instantly, across all assets, zero data migration.
- Keeps "mint is 100% local, zero node contact" (the property §5 concedes).
- No shard-store DoS surface (no write-once/quota machinery needed), no per-node database, no
  backup lag, no DHT-resolution hop at open.
- Large n is cheap: even n=50 ≈ ~100 KB of metadata — so wide fan-out quorums are feasible
  without any node state.

**Cons**
- **Cannot host proactive refresh (PSS), ever:** published metadata is immutable — old-epoch
  shares can never be deleted/revoked. Refresh, share revocation, and post-mint holder
  expansion are cryptographically unavailable (see 14.4, the pivot).
- Membership is frozen per asset at mint; a retired node's identity (seed) must be preserved
  as a custody obligation forever, or its vote is lost for every asset sealed to it.
- Rotation control is limited to seed custody; ejecting a compromised member cannot invalidate
  shares already sealed to it (only the seed's secrecy protects them).
- Metadata carries per-asset crypto blobs (aesthetic/size cost — minor, ~1–2 KB per share).

### 14.3 Stateful — pros / cons

**Pros**
- **The only model that can host the PSS machinery** (agreed 2026-07-16): epoch refresh with
  old-share deletion, share revocation, post-mint holder expansion ("shard mining"), and —
  the synthesis — **self-healing**: a node that lost its store (or a fresh replacement) is
  re-issued its share via the same t-signed ceremony; shard loss stops being permanent while
  ≥ t holders survive. Rotation becomes a real incident response: eject + reshare ⇒ the
  ejected node's shares die with the old epoch.
- Membership can evolve per-KID without touching asset metadata (no re-publish, no new CID) —
  the requirement for decade-scale assets under node churn.
- Shares are not publicly harvestable (minor: they are PQ-hybrid sealed anyway).
- Slim, stable metadata (pins only).

**Cons**
- New availability domains: per-node shard stores (grow with every platform mint; ~2 GB per
  node per million assets) + DHT manifest resolution on every open.
- WITHOUT the PSS package, strictly worse than stateless: shard loss permanent, backup lag is
  a loss window, > n−t losses brick assets with all nodes alive ("worst of both worlds" — PO's
  original objection, conceded correct by all).
- Gives up zero-contact mint (mint pushes to n nodes; mitigable: async, idempotent,
  fire-and-confirm).
- New write-path attack surface (needs write-once, allow-list, quotas) and a node-to-node
  protocol where none exists today (nodes are currently blind to each other — a big lift).
- Operational burden: continuous store backup (until self-healing ships), storage accounting,
  migration tooling.

### 14.4 Discussion outcomes that inform the choice (recorded so they are not re-litigated)

1. **The P0 reframe.** The observed production instability (≈1-in-5 opens) is the confirmed
   CEK-commitment bug (brainstorming §11.0): opens silently require ALL n nodes, so fault
   tolerance is nominal. Under that bug more nodes make it WORSE. **Ship the P0 fix, then
   re-measure — the residual error rate is the true architectural signal**; today's numbers
   are noise.
2. **Threshold-first dispatch** (product owner, 2026-07-16): the client races eligible nodes
   at bounded concurrency (e.g. max-concurrency 5 over n candidates), fulfills at the first t,
   replaces failures from the remaining pool; all knobs in options. Correct in both models;
   invariant path item.
3. **The metadata-pinning concern, resolved:** "what if the IPFS node pinning the metadata
   goes down" applies equally to both models — metadata + ciphertext must be fetched for
   playback regardless, so asset-carried shares add zero marginal availability risk. The fix
   is content-plane replication (pin/receipts/repair — and the "dKMS pool as an IPFS-cluster"
   instinct = the availability provider's replication policy), which benefits both models.
4. **Trust model, resolved:** "trusted pool" is not an acceptable standing assumption — the
   threshold exists so approval failures below t simultaneous bad identities are
   non-catastrophic. Approval bounds membership; per-identity issuance caps + t-signed,
   audited share issuance bound the damage of approval failures (incl. multi-identity/sybil
   ownership, which approval workflows cannot see); rotation + proactive resharing is the
   incident response, not the prevention.
5. **The PSS pivot (the load-bearing fact):** proactive refresh derives its security from
   DELETING old shares. Immutable public metadata cannot delete ⇒ **PSS requires mutable,
   deletable, node-held custody state**. Therefore: stateful-without-PSS is the worst of both
   worlds; **stateful-with-PSS is self-healing; stateless can never have PSS.** If post-mint
   expansion/refresh is a firm requirement, the architecture question is effectively decided.
6. **Dealer-at-mint simplification:** no DKG is needed at mint in either model — the minter
   legitimately knows the CEK (it encrypted the content). MPC machinery is needed only
   post-mint (mining/repair/refresh), off the hot path.

### 14.5 Invariant improvement path (ships under EITHER outcome — plan and build now)

1. **P0 CEK-commitment fix** — bug, ships immediately; also the m==t integrity backstop §4
   requires under any custody model.
2. **General t-of-n crypto** (§4): split/combine, cheater detection, golden vectors.
3. **Auto-quorum policy** (§3): t, n derived in-code, capped/floored by config.
4. **Threshold-first fan-out dispatch** (14.4.2): max-concurrency, fulfill-at-t,
   replace-on-failure, parameters in options.
5. **Pool directory + approval workflow**: governance-signed member list, read-only at mint
   (who may ever be sealed to). Only its per-KID *write* path is model-dependent.
6. **Scoped access delegation v2** (§6) — binds a durable directory identifier under either
   model (dynamic n makes node-sets per-asset regardless); exact identifier if the DHT pool
   document is descoped: a signed quorum-descriptor set id.
7. **Recover-proof v2 hardening** (§8) — rides the same coordinated redeploy.
8. **Content-plane availability** (`2026-07-15-media-content-plane-adoption-design.md`) —
   metadata + media replication; prerequisite for fair re-measurement either way.

### 14.6 Divergent path A — stateless (asset-carried, generalized)

Metadata carries n sealed shares (n may be large); recover stays share-in-request; no node
state, no DHT manifest, no mint-push. Adds on top of §14.5: nothing — that is the point.
**Choose when:** post-mint holder expansion/refresh is NOT a firm requirement, and node
identities (seeds) can be treated as long-lived custody obligations. **Accept:** frozen
per-asset membership; no share revocation; churn beyond n−t permanent identity deaths bricks
an asset (mitigated only by choosing n large at mint).

### 14.7 Divergent path B — stateful + PSS (one bound package, phasing allowed)

The §5 machinery (shard-store, mint-push, recover-by-KID, KID-manifest) **bound to** the
proactive-secret-sharing package — the store never ships without at least a committed,
scheduled path to:
- **Share redistribution** (post-mint mining/repair): VSS-verified, t-signed issuance;
  one share per node identity per KID; audit trail in the manifest; ceremony never
  reconstructs the CEK (Desmedt–Jajodia '97; MPSS, Schultz–Liskov '10).
- **Epoch refresh + deletion** (rotation/incident response): HJKY '95 proactive refresh.
- **Verification layer:** hash-based commitments (extend the `cek_commitment` pattern), NOT
  DLog-based Feldman/Pedersen, to preserve the platform's PQ posture.
- **References for the ADR:** ISO/IEC 19592-2 (secret sharing), NIST IR 8214/8214A–C
  (multi-party threshold crypto), RFC 9591 (FROST — ceremony hygiene only; FROST itself is
  not a dKMS root per KEY_PROVIDER.md), drand/League of Entropy (operational reshare-on-
  membership-change template), Lit Protocol (domain precedent — see `lit-keystore-moleculer`),
  DEDIS `kyber` (reference implementation of VSS/DKG/PSS building blocks; expect to implement
  the Rust layer ourselves on `ddrm-envelope` primitives), RWOT8 "Shamir Secret Sharing Best
  Practices" draft (github.com/WebOfTrustInfo/rwot8-barcelona — reviewed 2026-07-16: its five
  pillars map onto us as envelope-encryption = our existing CEK model, threshold-by-fault-
  tolerance = §3, share verification = commitment/cross-check/VSS, PSS = this package; its
  "Centralized Management Plane" pillar must be read as central CONTROL/AUDIT — the governance
  directory + audited issuance — never a central party in the key path; note the pillar's PSS
  guidance silently assumes deletable node-held shares, i.e. statefulness).
- **Implementation notes (if chosen):** CBOR over JSON for shard records; embedded KV store
  (e.g. LevelDB-class) over raw files for the shard-store.
**Choose when:** post-mint expansion ("shard mining"), share revocation, or self-healing
custody is a firm product requirement. **Accept:** n-node mint push, node-to-node protocol,
store operations until self-healing lands.

### 14.8 The deciding questions for the ADR (answer these, and the choice falls out)

1. **Is post-mint holder expansion / share revocation a firm product requirement?**
   Yes ⇒ path B (only it can host PSS). No ⇒ path A is simpler and strictly cheaper.
2. **What is the residual open-failure rate after the P0 fix + content-plane availability
   land?** Re-measure before attributing anything to architecture.
3. **Asset lifetime vs churn horizon:** how long must sealed assets outlive quorum
   membership, and when do third-party operators join? (The trigger that makes B's
   obligations pay for themselves.)
