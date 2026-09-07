# custody-host — Production is NOT Docker. Read this first.

This directory builds and runs a **simulation** of the dKMS custody-node
topology for the installed e2e proof (ELACITY-2298). It is a
production-*like* container image and bring-up harness, not a production
deployment artifact, and it is not intended to become one.

## What "simulation" means here

A real custody pool is **three physically distinct machines, run by three
distinct operators**, on separate networks, with separate failure domains
(power, hosting provider, jurisdiction). That physical and organizational
separation is the actual security property a 2-of-3 custody committee buys:
no single compromised host, operator, or datacenter can reconstruct a
recipient's key contribution alone.

Docker Compose on one laptop or one CI runner cannot simulate that
separation — there is one kernel, one disk, one operator (you) with root on
all three "nodes" at once. What this harness *does* faithfully simulate:

- **Process isolation**: each node is its own container, own PID namespace,
  own filesystem, own non-root user, cannot read another node's data root.
- **Real Carrier transport**: nodes talk to each other over actual Carrier
  QUIC connections on the container network, not in-process function calls —
  the same wire protocol production nodes use.
- **Node loss**: stopping/killing a container is a real process death a real
  custody node could suffer, exercised the same way the production runbook
  would exercise it.

What it does **not** simulate, and never will: distinct hardware, distinct
operators, distinct jurisdictions, or any of the trust-boundary separation
those provide. Do not point this Dockerfile, this compose file, or this
image at anything a real recipient's key material will ever touch.

## What the image contains

Five binaries built directly with `cargo build --release` (not
`scripts/setup-source-home.sh`, which installs the full ~30-binary/33-capsule
home topology this role does not need), plus one fetched pre-built binary:

- `elastos` — the runtime CLI (`identity show`, `protected-content-config
  provision-custody-node`, `run`).
- `custody-provider` — the shard-holder plane: verifies signed release
  operations/rights evidence, returns recipient-sealed contributions. Never
  reconstructs a CEK; that happens on the playback runtime, not here.
- `chain-provider` — the rights-evidence plane the shard-holder settles
  every release against: `evaluate` prepares the rights request, asks this
  node's own `elastos://chain` for `protected_content_rights_evidence`, then
  settles. A node without it fails every release closed, so `elastos run`
  refuses to start the chain plane without the network configuration
  described under "The `/shared` handoff" below. It verifies the signed
  Runtime release operations it evaluates against the same trusted Runtime
  issuer the custody state was provisioned with (`CUSTODY_TRUSTED_RUNTIME_ISSUER`
  on first boot), never this container's own device key.
- `availability-provider`, `ipfs-provider` — hosted alongside custody so this
  container also serves as a content-replica peer.
- `kubo` — the IPFS daemon `ipfs-provider`'s replica plane shells out to for
  its add/pin path; it hard-errors without one
  (`capsules/ipfs-provider/src/main.rs` `ensure_kubo()`). Not a Cargo build
  (kubo is a separate Go project): the builder stage fetches a pinned,
  checksum-verified official release tarball from dist.ipfs.tech instead.
  It ships at the fixed path `/usr/local/bin/kubo`, outside the state
  volume, with `ELASTOS_IPFS_KUBO_PATH` baked into the image so
  `find_kubo_binary`'s first, highest-priority lookup finds it with no
  operator-supplied env.

Runtime base is `busybox:glibc` (~5MB), not `debian:bookworm-slim`: no
apt/dpkg, no shell beyond busybox ash. See the Dockerfile and the task
report for the full build-strategy rationale (stripped binaries, BuildKit
cache mounts, the image-as-source-of-truth pattern for the data volume).

## Env contract

Exactly one variable is required on **every** boot:

- `ELASTOS_AVAILABILITY_ENSURE_URL` — the availability-provider plane has no
  CLI flag for its replication target, only an env var; `elastos run --with
  availability-provider` fails closed without one.

Three more are required on **first boot only** (the node's one-time
provisioning of its inactive custody state and public descriptor):

- `CUSTODY_TRUSTED_RUNTIME_ISSUER`, `CUSTODY_OPERATOR`, `CUSTODY_FAILURE_DOMAIN`

None of the four has an image-level `ENV`/`ARG` default. After first boot,
`ELASTOS_AVAILABILITY_ENSURE_URL` is persisted into the node's owner-only
init receipt and restored from it automatically — **a restart of an
already-provisioned node needs zero env vars**. The `CUSTODY_*` trio is
truly first-boot-only: `provision-custody-node` runs exactly once per data
volume.

## The `/shared` handoff

The node's public descriptor is exported to `/shared/<did>.descriptor.json`,
named by the node's own DID (not an operator-supplied index) — the
descriptor's own `transport.peer_did` field already carries the DID, so
there is no separate `.did` file. This is the only thing that crosses out of
the container's private, owner-only data root. It is written owner-only
(`0600`, the container's uid) and stays that way: the composition ceremony
refuses a descriptor that is not owner-only.

Bind-mount ownership differs by host. The containers run as an unprivileged
uid (10001) that does not exist on the host, and a bind mount keeps the host
directory's owner and mode. Docker Desktop on macOS maps ownership so the
nodes can write to `shared/` and the host can read their files regardless; a
Linux host (a CI runner, a bare server) does neither. `up.sh up` therefore
makes `shared/` world-writable with the sticky bit (`chmod 1777 shared`)
before the containers start, the entrypoint probes `/shared` for
writability up front and fails closed with that remedy instead of failing
the descriptor export on every boot; `shared/chain-provider.json`, the one
file crossing the other way, is written `0644` so the nodes' user can read
it (it holds network ids, contract addresses and RPC URLs, nothing secret;
a keyed RPC URL does not belong in a simulation harness), and the
entrypoint checks that readability up front with the same kind of remedy;
and once a node is ready `up.sh`
streams its descriptor out through an exec as the node's own user
(`docker compose exec cat`; `compose cp` would read the DID's colons as a
service separator) into a file this host user creates owner-only, then
replaces the node's copy with it, so the ceremony and the proof driver
read it on either host.

One thing crosses **in**: `/shared/chain-provider.json`, the client's
protected-content network configuration (the same schema as the client's
`protected-content/chain-provider.json`) with evidence RPC URLs that this
container can reach. The entrypoint syncs it into the node's private
`protected-content/chain-provider.json` on every boot (image-style: a
changed file takes effect on the next restart) and fails closed with the
remedy when it is missing. `up.sh up` derives it from the client data dir
(`up.sh sync-chain-config [CLIENT_DATA_DIR]` re-derives it later), rewriting
host-loopback URLs (`127.0.0.1`, `localhost` — a local Anvil fork and its
distinct-origin alias) to `host.docker.internal` (Docker Desktop resolves
that name on its own; for a Linux engine the compose file maps it to the
engine's `host-gateway`), and naming that one host in the network's `plain_http_rpc_hosts`
allowlist (the chain-provider admits plain `http://` only against loopback
unless a host is allowlisted explicitly; the list is empty by default);
every other URL passes through unchanged. When the client has no
chain configuration yet (the CI ceremony brings the nodes up before the
client is provisioned), `up` falls back to the proof driver's placeholder
network with a loud NOTE — such nodes cannot settle a release until the real
file is synced and they are restarted.

## 3-node bring-up

Composing three of these into an actual (simulated) custody pool —
`docker-compose.yml` plus an `up.sh` orchestration script — is a separate
task; the files land alongside this one when that work merges.
