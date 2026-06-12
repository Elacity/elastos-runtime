# dKMS 2-of-3 Deployment Kit

A standardized, deployment-ready kit for standing up the PQ-hybrid threshold dKMS as a **2-of-3
quorum** across your three nodes, and cutting the ElastOS runtime over to it. Everything is grounded
in the real capsule code — no invented surfaces — and the full path is **verified offline today**
(`scripts/ddrm-consumer-dkms-quorum-smoke.sh` → PASS, including DKG + attestation).

It is **100% ready to run the moment your node details land**. Nothing here depends on those
details; the only remaining input is [INTAKE.md](./INTAKE.md).

## Contents

| File | What it is |
|---|---|
| [`RUNBOOK.md`](./RUNBOOK.md) | The step-by-step deployment: topology, identities, WireGuard, firewall, per-node install, descriptor assembly, cutover, verification, rollback, go-live gate. |
| [`INTAKE.md`](./INTAKE.md) | The exact node details + decisions I need from you to go live. |
| [`SCALING_AND_INTEROP.md`](./SCALING_AND_INTEROP.md) | Briefing: scaling beyond 3 nodes, how nodes interact (and why not Carrier), ela.city interoperability, limits/risks/wins. |
| [`LIFECYCLE_AND_FUTUREPROOFING.md`](./LIFECYCLE_AND_FUTUREPROOFING.md) | Lock-in/rotation analysis: do today's 2-of-3 assets survive adding/rotating nodes? DAO-council mapping, transport vs. runtime principles, ela.city playback path. |
| [`DKG_CEREMONY.md`](./DKG_CEREMONY.md) | The born-distributed key-generation procedure (advanced / opt-in). |
| [`configs/node.env.template`](./configs/node.env.template) | Per-node systemd `EnvironmentFile` (the 4 env vars the node reads). |
| [`configs/dkms-authority.v2.json.template`](./configs/dkms-authority.v2.json.template) | The public-only `elastos.dkms.authority/v2` descriptor (runtime host). |
| [`configs/key-provider.init.json.template`](./configs/key-provider.init.json.template) | Runtime key-provider init config that cuts the rail over to the quorum. |
| [`systemd/dkms-authority.service`](./systemd/dkms-authority.service) | Hardened systemd unit for the node daemon. |
| [`bin/dkms-validate-descriptor.py`](./bin/dkms-validate-descriptor.py) | Offline descriptor schema validator (mirrors the runtime parser exactly). |
| [`bin/dkms-preflight.sh`](./bin/dkms-preflight.sh) | Preflight checker: `identity` (provision/read a node), `node` (node-host env), `runtime` (descriptor + reachability). |
| `capsules/dkms-keygen` (in-tree tool) | Operator-console keygen for the operator + runtime-caller ML-DSA-65 identities (single source of truth via `ddrm-envelope`); self-test built in. Used in RUNBOOK §5. |

## The 60-second mental model

- **3 nodes**, each holding a secret master seed → a stable public identity. **Any 2** reconstruct a
  CEK; **no 1** ever holds it; the rail **survives one dead node**.
- The **runtime is a client**: it holds a **public-only descriptor** (pins + endpoints) and its
  **own caller identity**. It never holds a master or a CEK — it delegates recovery, exactly as PC2
  delegates to the Lit network.
- **Transport security is the app-layer PQ channel** (attested channel key, sealed replay-bound
  frames), with **WireGuard** underneath for access control. No "TLS-off HTTPS" like PC2.
- **Cutover is config-only** — flip `backend` + point at the descriptor; same binary, same flow.

## Verified-offline status

| Check | Tool | Result |
|---|---|---|
| 2-of-3 quorum open, any-2 failover, below-quorum fail-closed | `ddrm-consumer-dkms-quorum-smoke.sh` | PASS |
| DKG born-distributed generation (gates 49–51) | same | PASS |
| Portable threshold attestation `QuorumReleaseProofV1` (gates 52–54) | same | PASS |
| Real TCP transport + encrypted/authenticated channel gates | `ddrm-consumer-dkms-tcp-smoke.sh` | PASS path |
| Descriptor schema (real + negative cases) | `bin/dkms-validate-descriptor.py` | PASS |

Next: fill [INTAKE.md](./INTAKE.md) → deploy.
