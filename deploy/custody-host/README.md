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

One variable is required on **every** boot **in the `storage` role only**:

- `ELASTOS_AVAILABILITY_ENSURE_URL` — the availability-provider plane has no
  CLI flag for its replication target, only an env var; `elastos run --with
  availability-provider` fails closed without one. A `custody`-role node does
  not host that plane and never needs it (see [Node roles](#node-roles)). It is
  still persisted into the init receipt whenever supplied, so a node first
  provisioned as custody-only can later be restarted as storage without
  supplying it again; conversely a value in the environment overrides the
  persisted one.

Three more are required on **first boot only** (the node's one-time
provisioning of its inactive custody state and public descriptor):

- `CUSTODY_TRUSTED_RUNTIME_ISSUER`, `CUSTODY_OPERATOR`, `CUSTODY_FAILURE_DOMAIN`

One env var is always optional: `CUSTODY_NODE_LOG` (defaults to `debug`).
The node's role is not an env var — it is the `--role` option (see
[Node roles](#node-roles)).

None of the four required vars has an image-level `ENV`/`ARG` default. After first boot,
`ELASTOS_AVAILABILITY_ENSURE_URL` is persisted into the node's owner-only
init receipt and restored from it automatically — **a restart of an
already-provisioned node needs zero env vars**. The `CUSTODY_*` trio is
truly first-boot-only: `provision-custody-node` runs exactly once per data
volume.

## Node roles

The image's entrypoint takes `--role <storage|custody>`, which selects which
provider planes a node hosts:

| role | planes | content replica? |
| --- | --- | --- |
| `custody` (**entrypoint default**) | custody + chain | no — runs no kubo |
| `storage` | custody + chain + availability + ipfs | yes |

`custody` is the default because custody *is* the role: a node has to be asked,
explicitly, to take on storage as well. The option reaches the entrypoint as a
container argument — the image's `ENTRYPOINT` is the script, so `command:` in
`docker-compose.yml` (or trailing args to `docker run`) become its `"$@"`:

```yaml
command: ["--role", "storage"]
```

**This harness overrides the default to `storage`** (see the comment on
`custody-a` in `docker-compose.yml`) — see "Why the harness asks for storage"
below. To exercise the custody-only path instead:

```bash
CUSTODY_HOST_ROLE=custody ./up.sh up        # all three nodes custody-only
```

**Why the role exists.** Key-share custody and release is the custody provider's
entire job — its whole operation surface is `init`/`status`/`shutdown`,
`provision_node_share`, `release_contribution`, `prepare_evidence`,
`settle_evidence`. It never reads ciphertext: the CEK envelope arrives inside
the signed release operation, so nothing in the release path fetches content.
A custody-only node therefore has no reason to run kubo, and — more
importantly — must never be a content-availability failure point. Before the
role existed, every registered peer was conscripted as a ciphertext replica,
and a custody node that could not pin would wedge a mint indefinitely.

**`--with chain-provider` is mandatory in both roles.** A committee member
settles every release through its own chain rights evidence, so a node without
the chain plane fails closed on release.

**Why the harness asks for storage.** A protected-content mint needs
the publisher's own local pin *plus two proven remote replicas*
(`PROTECTED_CONTENT_MIN_REPLICAS = 3` and
`PROTECTED_CONTENT_REQUIRE_LIVE_MULTI_PEER_PROOF = true`, both hardcoded in
`elastos/crates/elastos-server/src/protected_content_runtime.rs`). These three
nodes are the only peers the harness has, so with all three custody-only no
mint can complete. Run `CUSTODY_HOST_ROLE=custody` to exercise the custody-only
path; expect availability to stay unsatisfied unless you add a separate storage
node.

**Changing the role needs a rebuild.** `entrypoint.sh` is baked into the image
(`COPY … /entrypoint.sh`), so editing it on disk does nothing to a running
node — `./up.sh up` rebuilds, a bare `docker compose up -d` does not.

## Log level

The nodes default to `debug`. To change it:

```bash
# at bring-up
./up.sh up --log-level trace
./up.sh up --log-level trace "$HOME/Library/Application Support/elastos"

# on already-running nodes (recreates the containers; state is preserved)
./up.sh log-level trace
./up.sh log-level info
```

`LEVEL` is one of `error|warn|info|debug|trace`, or a raw `RUST_LOG` filter
passed through verbatim for surgical cases:

```bash
./up.sh log-level 'elastos_runtime::provider::bridge=trace,elastos=debug'
```

The chosen value is written to `.env` as `CUSTODY_NODE_LOG`, which
`docker-compose.yml` substitutes into each node's `RUST_LOG`. It persists
across `up`, `down` and `log-level`; only `destroy` (which removes `.env`'s
companion state) or an explicit new value replaces it. Precedence at `up`:
`--log-level` flag, then a `CUSTODY_NODE_LOG` already in the environment,
then whatever `.env` carries from a previous run, then the compose default.

**Why a level word expands to three directives.** `elastos-server` builds its
filter as `EnvFilter::from_default_env().add_directive("elastos=info")`
(`elastos/crates/elastos-server/src/main.rs:1223-1224`). `add_directive`
*replaces* any directive parsed from `RUST_LOG` that has the same target, so
`RUST_LOG=trace` and even `RUST_LOG=elastos=trace` are both silently clobbered
back to `info`. Only a strictly longer target prefix out-ranks the baseline.
A level word therefore expands to all three namespaces the nodes log under:

| target | crate | examples |
| --- | --- | --- |
| `elastos::*` | the binary | `provider_host`, `server_infra` |
| `elastos_server::*` | server library | `carrier` |
| `elastos_runtime::*` | runtime library | `provider::bridge`, `provider::registry` |

`elastos_runtime` matters most in practice: `provider::bridge` is what reports
a provider operation that has not come back —

```
WARN elastos_runtime::provider::bridge: provider request still pending op=pin elapsed_secs=90
```

— and at `debug` it also emits the matching `provider request sent` /
`provider request settled op=… elapsed_ms=… ok=…` pair, which is how you tell
a slow operation from a hung one.

Following the logs:

```bash
docker compose -f deploy/custody-host/docker-compose.yml logs -f
docker logs -f custody-host-custody-c-1
```

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
