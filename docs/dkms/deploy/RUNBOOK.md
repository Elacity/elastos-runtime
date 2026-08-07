# dKMS 2-of-3 Quorum — Deployment Runbook

**Status:** deployment-ready. Everything here is grounded in the real capsule code
(`capsules/dkms-authority`, `capsules/key-provider`) and the runtime orchestrator
(`scripts/dev/ddrm-runtime-open`). The offline 2-of-3 dry-run (§11) **passes today** on a dev box,
including DKG born-distributed generation and portable threshold attestation. The only things this
runbook *cannot* fill in are your three node hosts' addresses and ports — those are the
[INTAKE](./INTAKE.md) checklist, the last step before we go live.

> **What the quorum gives you.** The content key (CEK) is split across **three independent
> secret-holding nodes**. **Any two** reconstruct it; **no single node** ever holds it, and the
> rail **survives one dead node**. This is the availability property PC2 rents from Lit's opaque
> BLS network — here `t`, `n`, the field arithmetic, the quorum policy, and the failover are
> explicit, owned, and gated.

---

## 0. Topology

```
                 WireGuard private mesh (10.66.0.0/24)
   ┌────────────────────────────────────────────────────────────────┐
   │                                                                  │
   │  Node A  (e.g. Interserver)   Node B (e.g. Contabo)   Node C (new)│
   │  10.66.0.2:9443               10.66.0.3:9443          10.66.0.4:9443
   │   dkms-authority               dkms-authority          dkms-authority
   │   master.seed (0600)           master.seed (0600)      master.seed (0600)
   │        ▲                              ▲                       ▲     │
   └────────┼──────────────────────────────┼───────────────────────┼────┘
            │   framed TCP + app-layer PQ-authenticated channel     │
            └──────────────┬───────────────┴───────────────────────┘
                           │
                    ElastOS runtime host
                    key-provider (backend=dkms)
                    holds: PUBLIC descriptor + its OWN caller seed
                    holds: NEVER a master, NEVER a CEK
```

Three nodes = three **independent failure domains** (different providers/regions is ideal).
The runtime is a **client**: it pins each node's public identity and delegates recovery. The
master seeds and the CEK never enter the runtime — exactly as PC2 holds only Lit's public
`pkpId`/`authority` and delegates to the Lit network.

---

## 1. The four identities (know these cold)

| Identity | Lives where | Secret? | Role |
|---|---|---|---|
| **Node master seed** (×3) | each node's `DKMS_AUTHORITY_KEY_STORE` | **secret, node-local** | derives that node's stable public identity; never leaves the node |
| **Node public identity** (×3) | the descriptor on the runtime | public | `verifying_key_b64` (seal trust) + `recipient_pub_b64` (escrow target) |
| **Runtime caller identity** | runtime's `dkms_caller_seed_b64` (seed); its **public VK** on every node's allow-list | seed is runtime-private | the node only serves this caller |
| **Operator identity** | operator console (signing key); its **public VK** pinned on every node | signing key off-node | authorizes lifecycle ops (rotate/revoke/reconfigure/DKG) |

Two facts that make this safe:
- The node identity is **deterministic** from the master seed — relaunching a node yields the
  byte-identical public identity (so escrows stay valid).
- The descriptor is **public-only**: the parser **rejects** any `authority_master_seed_b64`
  (the old v1 shape). The recovery secret must never reach the runtime.

---

## 2. Build the binaries (operator does this — you don't touch a terminal)

From the repo root:

```bash
cargo build --release --manifest-path capsules/dkms-authority/Cargo.toml
cargo build --release --manifest-path capsules/key-provider/Cargo.toml --features key-authority-ref
```

Ship `capsules/dkms-authority/target/release/dkms-authority` to each node host at
`/opt/elastos/bin/dkms-authority`. The `key-provider` ships with the runtime.

---

## 3. Private network — WireGuard mesh

The node does **not** terminate TLS, and it does not need to: every post-handshake frame is a
**sealed, mutually-authenticated, replay-bound** envelope (the node publishes a master-derived
channel KEM key **attested under its descriptor-pinned identity** at `hello`; a substituted key —
an attacker terminating the TCP connection — fails verification, fail-closed). That is the
"authenticated PQ channel". **Contrast PC2:** its dDRM boundary is HTTPS with
`rejectUnauthorized: false` — TLS verification off, channel authenticates nothing.

WireGuard sits **under** that as defense-in-depth + access control: it makes the dKMS port
unreachable except from the runtime and the peer nodes.

Per host (`/etc/wireguard/wg0.conf`), assign:
- Node A `10.66.0.2`, Node B `10.66.0.3`, Node C `10.66.0.4`, runtime `10.66.0.1`.
- Each node peers with the runtime (and optionally each other, required for the DKG ceremony §10).

Bring it up: `systemctl enable --now wg-quick@wg0`.

---

## 4. Firewall (each node)

Only WireGuard is exposed to the internet; the dKMS port lives **only** on the `wg0` interface.

```bash
# allow WireGuard (its UDP port) from anywhere
ufw allow 51820/udp
# allow the dKMS port ONLY on the wg0 interface
ufw allow in on wg0 to any port 9443 proto tcp
# default deny inbound
ufw default deny incoming
ufw enable
```

Set `DKMS_AUTHORITY_LISTEN=tcp:10.66.0.X:9443` (bind the **WireGuard IP**, never `0.0.0.0`).

---

## 5. Generate the operator + runtime-caller identities (operator console, off-node)

Both are ML-DSA-65 keypairs derived from a 32-byte seed. Use the shipped operator tool
**`dkms-keygen`** (`capsules/dkms-keygen` — it calls the SAME
`ddrm_envelope::seal::mldsa_seal_keypair` the node + key-provider use, so the VK is byte-identical
to what the node accepts). Build once: `cargo build --release --manifest-path capsules/dkms-keygen/Cargo.toml`.

```bash
# Operator identity — seed stays on the console, VK pinned on every node.
dkms-keygen keygen --role operator --out /etc/elastos/operator
#   → operator.seed (0600, SECRET) + operator.vk   → __OPERATOR_VK_B64__

# Runtime caller identity — seed → runtime config, VK → every node's allow-list.
dkms-keygen keygen --role caller --out /etc/elastos/caller
#   → caller.seed (0600, → dkms_caller_seed_b64) + caller.vk → __RUNTIME_CALLER_VK_B64__
```

- **Operator:** keep `operator.seed` on the operator console (it is the signing key). Pin
  `operator.vk` (`__OPERATOR_VK_B64__`) into every node's `DKMS_AUTHORITY_OPERATOR_VK`.
- **Runtime caller:** `caller.seed` becomes the runtime's `dkms_caller_seed_b64`; `caller.vk`
  (`__RUNTIME_CALLER_VK_B64__`) goes on every node's `DKMS_AUTHORITY_ALLOWED_CALLERS`.

> **Self-test (do this):** `dkms-keygen derive-vk --seed-b64 <caller seed>` must reprint
> `caller.vk` exactly. That proves the seed→VK derivation is the deterministic one the node's
> allow-list matches against — if it ever differs, the node would refuse the caller.
>
> **Council rotation:** to hand the operator role to a newly-elected DAO council, mint a fresh
> operator identity and rotate `DKMS_AUTHORITY_OPERATOR_VK` on each node (an operator-authorized
> lifecycle step). The *node* identities and escrowed content are untouched.

---

## 6. Per-node install + provision the identity

On **each** node host:

1. Place the binary at `/opt/elastos/bin/dkms-authority` (`chmod 755`).
2. Create the service account + state dir:
   ```bash
   useradd --system --home /var/lib/elastos/dkms dkms
   install -d -o dkms -g dkms -m 0700 /var/lib/elastos/dkms
   ```
3. Copy `configs/node.env.template` → `/etc/elastos/dkms-authority.env`, fill the placeholders
   (`DKMS_AUTHORITY_LISTEN`, `_KEY_STORE`, `_ALLOWED_CALLERS=__RUNTIME_CALLER_VK_B64__`,
   `_OPERATOR_VK=__OPERATOR_VK_B64__`). `chmod 600`, `chown dkms`.
4. **Provision + read this node's public identity** (idempotent; creates the master on first run):
   ```bash
   docs/dkms/deploy/bin/dkms-preflight.sh identity \
       /opt/elastos/bin/dkms-authority /var/lib/elastos/dkms/master.seed
   ```
   It prints `{verifying_key_b64, recipient_pub_b64, authority_endpoint:"tcp:REPLACE…"}`. Record
   that block for this node, and set `authority_endpoint` to this node's `tcp:10.66.0.X:9443`.

Repeat for nodes A, B, C → you now have three public identity blocks.

---

## 7. Assemble + validate the descriptor (runtime host)

Copy `configs/dkms-authority.v2.json.template` → `/etc/elastos/dkms-authority.v2.json` and paste
the three blocks (node A goes BOTH at top-level and as `threshold.nodes[0]`). Then validate
**offline**:

```bash
docs/dkms/deploy/bin/dkms-validate-descriptor.py \
    /etc/elastos/dkms-authority.v2.json --require-tcp
```

Must print `VALID … 2-of-3`. This mirrors the runtime parser exactly (schema, `t==2`, 2-or-3
distinct nodes, `nodes[0]` matches top-level, no secret fields). If it fails, the runtime would
fail closed too — fix before proceeding.

---

## 8. Start the daemons + node-host preflight

On each node:

```bash
install -m 0644 docs/dkms/deploy/systemd/dkms-authority.service \
    /etc/systemd/system/dkms-authority.service
systemctl daemon-reload
systemctl enable --now dkms-authority

# Preflight the node host (env + binary + identity + store perms):
docs/dkms/deploy/bin/dkms-preflight.sh node \
    /etc/elastos/dkms-authority.env /opt/elastos/bin/dkms-authority \
    --expect-caller __RUNTIME_CALLER_VK_B64__ --expect-operator __OPERATOR_VK_B64__
```

Healthy startup logs (from the binary):
```
dkms-authority: enforcing a 1-entry caller allow-list
dkms-authority: operator identity pinned (lifecycle ops enabled)
dkms-authority: listening on tcp:10.66.0.X:9443
```
All three lines **must** appear. "enforcing a … allow-list" and "operator identity pinned" confirm
the node will refuse unknown callers and accept lifecycle authorization. No allow-list line = the
node would serve anyone → stop and fix the env.

---

## 9. Runtime cutover (local → 2-of-3)

The cutover is a **config change only** — same key-provider binary, same open flow; only
`authority.backend` + the descriptor change (this is the design property the smoke proves by
flipping backends without touching the flow).

1. Copy `configs/key-provider.init.json.template` → the runtime's key-provider init config; set
   `dkms_authority_descriptor` to `/etc/elastos/dkms-authority.v2.json` and
   `dkms_caller_seed_b64` to the runtime caller seed from §5.
2. Runtime-host preflight (validates descriptor + init config + probes every node endpoint):
   ```bash
   docs/dkms/deploy/bin/dkms-preflight.sh runtime \
       /etc/elastos/dkms-authority.v2.json /etc/elastos/key-provider.init.json --require-tcp
   ```
   Every node endpoint must report `reachable`.
3. Restart the runtime so key-provider re-inits with `backend=dkms`.

The publish side then Shamir-splits each CEK over GF(256) into three indexed shares and escrows one
to each node's `recipient_pub_b64`; the node-set identity (a hash over all three VKs + `t=2`) is
pinned so a later silently-swapped node fails closed.

---

## 10. Verify quorum + attestation (live)

After cutover, drive one real open and confirm:

- **Quorum release:** an open succeeds reaching **any two** of the three nodes.
- **Failover:** stop Node C (`systemctl stop dkms-authority`) → an open **still succeeds** (A+B
  serve). Stop a second node → the open **fails closed** (below quorum, no partial CEK, no record).
- **Attestation:** each releasing node co-signs a portable `QuorumReleaseProofV1` — it names the
  serving node-set, counts a real quorum, and binds the exact grant/session. A standalone verifier
  confirms it **offline from a file**, leaking zero key material. (This is gate 52–54 of the dry-run,
  which passes today.)

The **born-distributed DKG ceremony** (a CEK that exists *nowhere* even at birth, dealt across the
three nodes) is the higher-assurance option — see [DKG_CEREMONY.md](./DKG_CEREMONY.md). The
producer-escrow quorum above is the baseline; DKG is opt-in per key.

---

## 11. Offline dry-run (already verified)

Before any node details exist, the **entire** 2-of-3 path is exercisable offline against three
real `dkms-authority` daemons:

```bash
scripts/ddrm-consumer-dkms-quorum-smoke.sh      # 2-of-3 quorum, any-2 failover, DKG, attestation
scripts/ddrm-consumer-dkms-tcp-smoke.sh         # 2-of-2 over REAL TCP + the encrypted channel gates
```

Last run on this box: **PASS** — `ddrm-consumer-smoke (authority=dkms, 2-of-3 quorum): PASS`, with
DKG (gates 49–51) and threshold attestation (gates 52–54) green. The TCP smoke proves the exact
network transport + channel the production nodes use (plaintext recover refused, downgrade dropped,
MITM-tampered frame dropped, wrong channel key refused).

---

## 12. Rollback

Cutover is config-only, so rollback is fast and safe:

- **Runtime:** revert the key-provider init config to the previous backend (e.g. `reference` or a
  single-node `dkms`) and restart. Content escrowed to the quorum stays recoverable as long as the
  three node identities/stores are intact.
- **A bad node:** `systemctl stop dkms-authority` on the offender. With 2-of-3 the rail keeps
  serving on the other two. Restore the node from its **master.seed backup** to bring it back with
  the same identity — no re-escrow needed.
- **NEVER** delete a node's `master.seed`: it strands every CEK escrowed to that node's recipient.
  Restore from backup instead. (Re-keying content requires the operator-authorized reshare/rotate
  lifecycle, not a store wipe.)

---

## 13. Go-live gate

Proceed to production only when **all** are true:
- [ ] descriptor validates `--require-tcp` (§7)
- [ ] all three nodes' preflight `node` checks pass, all three startup-log lines present (§8)
- [ ] runtime preflight reports every endpoint reachable (§9)
- [ ] a live open succeeds, and survives one node down but fails closed below quorum (§10)
- [ ] each node's `master.seed` is backed up offline (encrypted) (§12)

The remaining inputs are in [INTAKE.md](./INTAKE.md).
