# Go-Live Intake — exactly what I need from you

Everything in this kit is ready **without** your live node details. This is the last step: fill
this in and we deploy. Nothing here is a secret you have to send me in plaintext — the only secrets
(node master seeds, the operator signing key, the runtime caller seed) are **generated on the
boxes** and never transit.

## A. The three node hosts

For each of your three nodes (Interserver, Contabo, and the third you'll spin up):

| # | Field | Example | Yours |
|---|---|---|---|
| 1 | Provider / region | Interserver / NJ | |
| 2 | SSH access (user@host) for the operator to install | `root@198.51.100.10` | |
| 3 | Public IP (for the WireGuard peer endpoint) | `198.51.100.10` | |
| 4 | WireGuard mesh IP I should assign | `10.66.0.2` | |
| 5 | dKMS listen port (on the wg IP) | `9443` | |
| 6 | OS + arch (for the right release binary) | Ubuntu 22.04 / x86_64 | |
| 7 | Can it reach the other two nodes over wg? (needed only for DKG) | yes | |

The runtime host (where `key-provider` runs) also needs a wg IP — default `10.66.0.1`.

## B. Decisions (pick, or take the default)

| Decision | Default | Notes |
|---|---|---|
| WireGuard subnet | `10.66.0.0/24` | any private /24 you don't already use |
| dKMS port | `9443` | only exposed on `wg0`, never public |
| Quorum mode per key | **producer-escrow 2-of-3** | baseline; DKG born-distributed is opt-in (see DKG_CEREMONY.md) |
| Operator console host | your laptop / a 4th box | holds the operator signing key; never on a node |

## C. What gets generated (not collected) — for your awareness

1. **Operator identity** — on the operator console. You keep the signing key; its public VK is
   pinned into all three nodes.
2. **Runtime caller identity** — on the runtime host. Seed → `dkms_caller_seed_b64`; its public VK
   → every node's allow-list.
3. **Each node's master seed + public identity** — created on first launch via
   `dkms-preflight.sh identity`. You back up each `master.seed` offline; you send me the **public**
   identity block for the descriptor.

## D. Tooling status

- **`dkms-keygen` operator helper — DONE (shipped).** `capsules/dkms-keygen` mints the operator +
  caller `(seed, VK)` pairs by calling the SAME `ddrm_envelope::seal::mldsa_seal_keypair` the node
  and key-provider use, with a built-in self-test (`derive-vk` reproduces the VK). Build:
  `cargo build --release --manifest-path capsules/dkms-keygen/Cargo.toml`. Used in RUNBOOK §5.
  *(It lives as a standalone tool rather than an `elastos` subcommand because the main `elastos`
  binary's iroh-pinned dependency tree conflicts with the PQ crypto crates; the deployment uses it
  directly on the operator console, which is exactly where it belongs.)*
- **`elastos dkms ceremony` coordinator** — only if you choose born-distributed DKG (§DKG doc). The
  ceremony is already implemented and verified end-to-end inside the runtime orchestrator; promoting
  it to a standalone operator command is the only remaining work, and it's optional for launch.

## E. The one product fork to confirm (from STRATEGIC_ROADMAP.md)

You already chose **native PQ end-to-end** as the north star. Confirm the dKMS deployment targets
**new/native content** first (Lit-escrowed legacy content keeps using the Lit compat path until
migrated). That keeps go-live scoped: stand up the quorum, point new publishes at it, migrate later.

---

When A is filled and B/E confirmed, I'll: assign wg IPs, generate the operator + caller identities,
walk each node through §6–§8 of the RUNBOOK, assemble + validate the descriptor, cut the runtime
over (§9), and run the live quorum/failover/attestation verification (§10).
