# dKMS Node Provisioning — how a real quorum node is laid out, and how to replicate it locally

Audience: an engineer who needs to (a) understand how the three LIVE dKMS quorum nodes
are provisioned, and (b) stand up an equivalent, fully isolated 3-node quorum on one
machine for development. Companion to [DKMS_OVER_CARRIER.md](DKMS_OVER_CARRIER.md)
(transport architecture + threat model). This document contains NO production
addresses, credentials, or seeds — it describes shape, not secrets.

---

## 0. Production is NOT Docker — read this first

The live quorum is three geographically distributed Linux servers (US / Europe / Asia),
each running `dkms-authority` **bare, as a systemd service**. Docker appears in this
document ONLY as a local simulation harness (§4): three containers standing in for
three servers on one laptop, because that gives each simulated node its own process
tree, filesystem, and network identity — the isolation you cannot get by running three
daemons out of one home directory. The binaries, env vars, on-disk seed layout, and
descriptor flow in the simulation are byte-identical to production; the only thing
Docker replaces is "three physical servers."

The `scripts/dev/run-creator-gateway.sh` LOCAL quorum (three daemons on one machine
over unix sockets) is the dev default so a producer vertical works offline. It is a
proof-of-protocol convenience, not the production shape.

## 1. What one live node is (anatomy)

Each quorum node runs exactly two dKMS processes, plus durable state:

| Piece | What it is | Where it lives (live layout) |
|---|---|---|
| `dkms-authority` | The node daemon. Owns this node's master seed; recovers + re-seals CEK shares in its own boundary. systemd service, unprivileged `dkms` user, enabled at boot. | binary `/opt/elastos/bin/dkms-authority` |
| master seed store | THE secret. Created once by the first `init`; every later boot deterministically re-derives the node's published identity from it. | `/var/lib/elastos/dkms/master.seed` (0600 `dkms:dkms`) |
| node env | Operator-pinned config (listen addr, store path, caller allow-list, read-only Base RPC pool for trustless grant checks). | `/etc/elastos/dkms-authority.env` (0600) |
| `dkms-carrier-node` | The Carrier (iroh) bridge: exposes a stable `did:key`, relays ciphertext byte-for-byte to the daemon's local socket. UNTRUSTED transport — the PQ-hybrid channel terminates end-to-end between the runtime's `key-provider` and `dkms-authority`. Additive: deploying it changes nothing about the daemon. | second process alongside the daemon; its own 32-byte identity seed (e.g. `/var/lib/elastos/dkms/carrier.seed`) |

Network posture on a live node: the daemon's TCP listener (`:9443`) is reachable ONLY
on a private mesh between the nodes — firewalled default-deny, never the public
internet. The Carrier bridge is what makes the node reachable by the runtime, by
`did:key`, NAT-traversed over relays — no VPN enrollment, no inbound port required.

### The daemon's env surface

```bash
# /etc/elastos/dkms-authority.env  (shape — values are per-deployment)
DKMS_AUTHORITY_LISTEN=tcp:<private-addr>:9443     # framed request/response listener
DKMS_AUTHORITY_KEY_STORE=/var/lib/elastos/dkms/master.seed
DKMS_AUTHORITY_ALLOWED_CALLERS=<caller VK b64>    # runtime caller allow-list (comma-sep)
DKMS_AUTHORITY_OPERATOR_VK=<operator VK b64>      # lifecycle authority (rotation)
# Trustless AccessGrant authorization — the node re-checks hasAccessByContentId ITSELF
# against its own pinned, read-only RPC pool (never trusting the caller's word):
DKMS_CHAIN_RPC_POOL=https://mainnet.base.org,<more RPCs>
# DKMS_RIGHTS_CONTRACT / DKMS_RIGHTS_SELECTOR / DKMS_CHAIN_ID default to live Base values.
```

### The bridge's env surface

```bash
DKMS_CARRIER_NODE_TARGET=<daemon addr>:9443   # the local dkms-authority to front
DKMS_CARRIER_NODE_SEED=/var/lib/elastos/dkms/carrier.seed
# prints its did:key on stdout at boot — that string goes into the descriptor
```

## 2. What is secret vs. what is shareable

| Artifact | Class | Notes |
|---|---|---|
| `master.seed` (per node) | **SECRET — never leaves the node** | Loss of a node's seed = loss of that node's identity + escrowed shares. Backed up encrypted, offline, by the operator. |
| `carrier.seed` (per node) | Secret (node-local) | Only pins the bridge's `did:key`; regenerating it just changes the node's carrier address (descriptor must be re-issued). |
| `caller.seed` (runtime) | **SECRET** | The runtime's caller identity; its VK is what nodes allow-list. |
| operator seed | **SECRET** | Lifecycle authority for rotation. |
| `dkms-authority.carrier.json` | **Public** | Verifying keys + escrow recipient keys + `carrier:did:key:…` endpoints. Safe to share — it contains exactly what a client must PIN, nothing a client could abuse. |

Why the quorum survives node churn: the CEK is Shamir-split 2-of-3, each indexed share
escrowed to a node's PUBLISHED recipient key. Any 2 nodes recover; one node down (or
one seed lost) is degraded, not fatal. Rotation re-shares without ever reconstructing
the master outside a node boundary. Growing to 5-of-7 etc. is a descriptor + re-share
operation, not a redesign.

## 3. How the three live nodes were provisioned (repeatable recipe)

Per node, in order — this is the whole ceremony:

1. **Install binaries**: build `dkms-authority` (release, default features — the secure
   build; `dev-modes` is hard-forbidden in release) and `dkms-carrier-node`; place in
   `/opt/elastos/bin/`.
2. **Create the service user + state dir**: `dkms` user, `/var/lib/elastos/dkms/` (0700).
3. **First `init` creates the master seed**: the daemon's first initialization against an
   empty store path mints the seed and publishes the node's PUBLIC identity
   (`seal_verifying_key_b64` + `seal_recipient_pub_b64`). Idempotent: re-running init
   against an existing store re-derives the SAME identity.
4. **Write the env file** (§1) and a systemd unit for `dkms-authority`; enable at boot.
5. **Start the Carrier bridge** (second unit) pointing at the daemon's listener; capture
   the `did:key` it prints.
6. **Assemble/refresh the public descriptor** from the three nodes' identities + dids —
   `scripts/dev/dkms-make-carrier-descriptor.py` rewrites endpoints to
   `carrier:did:key:…` while leaving the PQ pins untouched.
7. **Allow-list the runtime caller**: mint one caller identity with
   `dkms-keygen keygen --role caller` and put its VK in every node's
   `DKMS_AUTHORITY_ALLOWED_CALLERS`.
8. **Back up the master seed** (encrypted, offline). This is the one artifact you cannot
   re-mint.

A runtime then consumes the quorum with four env vars:

```bash
export ELASTOS_DKMS_REMOTE=1
export ELASTOS_DKMS_CARRIER=1
export ELASTOS_DKMS_REMOTE_DESCRIPTOR=~/.elastos-dkms/dkms-authority.carrier.json
export ELASTOS_DKMS_REMOTE_CALLER_SEED=~/.elastos-dkms/secrets/caller.seed
./scripts/dev/run-creator-gateway.sh
```

## 4. Local replication: the 3-container simulation (`scripts/dev/dkms-docker/`)

One command stands up three isolated "servers" on your machine and produces a
descriptor + caller seed a runtime can consume:

```bash
cd scripts/dev/dkms-docker
./up.sh
```

What it does — the same ceremony as §3, per container:

- builds ONE image containing `dkms-authority`, `dkms-carrier-node`, `dkms-keygen`
  (secure default build, no `dev-modes`);
- each of the three containers runs the one-shot `init` (minting its own master seed in
  a PRIVATE named volume — the stand-in for `/var/lib/elastos/dkms/`), then the daemon
  on `tcp:0.0.0.0:9443` (private to the compose network, mirroring the live mesh
  posture), then the Carrier bridge, publishing only PUBLIC artifacts (identity JSON +
  `did:key`) to a shared folder;
- mints one caller identity and allow-lists its VK on all three nodes;
- assembles `shared/dkms-authority.carrier.json` — same schema as the live descriptor,
  three DISTINCT identities enforced.

Then point a runtime at it exactly as in §3 (the script prints the four exports).
Because transport is Carrier, the runtime does not care that the "nodes" are containers
— it dials three `did:key`s, same as production.

Lifecycle: `./up.sh down` stops the quorum (seeds persist — nodes come back with the
same identities, like real servers rebooting); `./up.sh destroy` deletes the volumes
(a brand-new quorum, new descriptor required).

Quorum-behavior checks worth running: stop one container (`docker compose stop
dkms-node2`) and confirm recover still works (2-of-3); stop two and confirm it fails
closed.

### No Docker? The repo already has a same-host harness

`scripts/dev/ddrm-producer-smoke/live-producer-carrier-verify.sh` runs the full
mint → escrow → 2-of-3 recover → decrypt vertical over Carrier with three local
daemons + bridges, no containers. It proves the protocol; the compose harness proves
the deployment shape.

## 5. FAQ (the questions this doc exists to answer)

- **"Are the 3 nodes all on one machine via `run-creator-gateway.sh`?"** No — that
  script's LOCAL mode is the offline dev default. The live quorum is three separate
  geo-distributed servers; the gateway consumes them via `ELASTOS_DKMS_REMOTE=1` +
  `ELASTOS_DKMS_CARRIER=1` and the public descriptor.
- **"Can I have `dkms-authority.carrier.json`?"** Yes — it is public by design (§2).
  Ask the operator; it is not committed to the repo because it names a specific live
  deployment, not because it is secret.
- **"How do standalone `dkms-authority` processes relate to a runtime?"** The runtime's
  `key-provider` reads the descriptor, dials each node (over Carrier via the
  `dkms-carrier-client` sidecar), and runs init/recover sessions over the end-to-end
  PQ channel. Nodes are related to a runtime ONLY by the descriptor + the caller
  allow-list — there is no other coupling, which is why containers simulate them
  faithfully.
- **"Who may run a node today?"** Gated/federated: nodes are operator-provisioned and
  the runtime caller is allow-listed. Expansion (partners/stakers, 5-of-7) is a
  descriptor + re-share operation on the same protocol.
