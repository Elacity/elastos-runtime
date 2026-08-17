# Elastos Carrier in This Runtime

> Supplemental terminology note.
>
> Read the [repository README](../README.md) and
> [ARCHITECTURE.md](ARCHITECTURE.md) first.
> This file narrows the Carrier concept and its placement in the runtime. It is
> not the current shipped-behavior contract. For current behavior and proof
> levels, use [../state.md](../state.md), [COMMAND_MATRIX.md](COMMAND_MATRIX.md), and
> [RUNTIME_REPO_USER_STORY_CHECKLIST.md](RUNTIME_REPO_USER_STORY_CHECKLIST.md).

## What Carrier Is

**Carrier is the endpoint-authenticated off-box communication and content transport of
an ElastOS node.** It can carry peer discovery, messaging, streams,
replication, and peer-to-peer content transfer when Runtime routing selects it.

Carrier is not the whole Runtime, the capsule API, or the authority system. The
Runtime enforces principals, capabilities, sessions, routing, lifecycle, and
audit around it. Carrier endpoint authentication proves the transport peer; it
does not by itself prove who authored an application message. The transport
implementation may change without changing capsule code.

## Capsule Model

A capsule requests a typed Runtime resource operation. It does not call Carrier
or select a transport. Runtime may satisfy the request locally, dispatch it to a
provider, or select Carrier for an off-box route. Moving the target must not
change capsule code or expose tickets, peer endpoints, or raw sockets.

Carrier remains location-explicit transport inside Runtime and provider
implementations. Remote failure must stay visible as a typed operation result;
it must not become transparent remote-object behavior or a silent local
fallback.

### Signed Collaboration Bootstrap

A verified signed collaboration-network profile supplies bounded bootstrap
peers and authenticates the content-addressed default-conversation grant. The
Runtime collaboration service constructs one durable core and transport driver;
the driver hands Carrier only opaque signed envelopes and treats broadcast as a
transport observation, never product acceptance. Configured Chat text receives
typed projections through the Runtime-owned product port and never receives
tickets, decoded endpoints, raw sockets, or Carrier topics. The former
route-owned Room gossip exception no longer exists. People/discovery migration
remains separate open work.

```
┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│  chat (TUI)  │  │ chat-room    │  │  agent       │
│  (ratatui)   │  │ (web)        │  │  (headless)  │
└──────┬───────┘  └──────┬───────┘  └──────┬───────┘
       │                 │                 │
       └────────┬────────┴────────┬────────┘
                │  Runtime APIs   │
                │  same policy    │
                ▼                 ▼
┌────────────────────────────────────────────────────┐
│  Runtime (one per machine)                         │
│  ├── Carrier (one iroh endpoint, one DID)          │
│  │   └── Gossip buffer (shared by all capsules)    │
│  ├── Providers (did, peer, ai, localhost, ipfs*)   │
│  └── Capabilities + Sessions + Audit               │
└────────────────────────────────────────────────────┘
```

`ipfs*` is the current low-level content backend, not the intended app-facing
content contract.

**Same machine:** Chat surfaces share one runtime, one Carrier node, and one
gossip buffer. Messages between native Chat and Chat Room on the same machine
go through the shared Runtime buffer, not a separate network path.

**Cross machine:** Each machine has its own runtime and Carrier node with its own DID. Messages travel via iroh gossip mesh (QUIC + DHT + relay). From the capsule's perspective, this is invisible — `peer/gossip_send` works the same way.

```
Jetson                    WSL                     Laptop
Runtime A                 Runtime B               Runtime C
DID: did:key:z6Mk...     DID: did:key:z6Mn...    DID: did:key:z6Mp...
Carrier (iroh)            Carrier (iroh)          Carrier (iroh)
    │                         │                       │
    └─────────────────────────┴───────────────────────┘
                     iroh gossip mesh
```

## Technical Detail

```
ElastOS Node
├── Node Core / Runtime
│   ├── capabilities
│   ├── sessions
│   ├── provider dispatch
│   ├── audit
│   └── capsule lifecycle
├── Carrier
│   ├── communication (gossip, peer discovery, relay)
│   └── content transport (peer-to-peer fetch / serve)
└── Providers + Capsules
    └── capsule-facing `elastos://` and `localhost://` contracts
```

### Carrier vs. Elastos Carrier Native / Boson

The Elastos Foundation's Carrier (v1: C SDK, v2: DHT+services) and Boson (permissionless fork) are the closest historical analogs. In this runtime's model they are candidate backend implementations for the Carrier substrate. The contract stays stable; the transport can change.

### Carrier vs. `elastos://`

`elastos://` is the native namespace exposed to capsules and users.

Carrier is not identical to `elastos://`:

- Carrier gives decentralized peer/content semantics to the relevant parts of `elastos://`
- the runtime routes and authorizes `elastos://` operations
- providers define the meaning of subspaces such as `elastos://peer/`, `elastos://did/`, `elastos://chain/`, and `elastos://ai/`

Clean mental model:

- `elastos://` = namespace / contract surface
- Carrier = decentralized substrate behind peer/content operations
- runtime = trusted node core that hosts Carrier and enforces policy

### Carrier vs. IPLD, IPFS, and Availability

IPLD is not Carrier. IPLD is the content-addressed object graph/data model used
to represent linked SmartWeb objects, manifests, signed heads, provenance, and
availability receipts.

IPFS/Kubo is not Carrier either. It is the first block/CID backend used by the
current `ipfs-provider`.

The clean content model is:

```text
capsule -> runtime capability -> elastos://content/* provider
        -> Carrier coordination/transport where needed
        -> IPFS/Kubo/Elacity/supernode/volunteer storage backends
        -> signed availability receipt
```

Carrier should make content exchange feel like one SmartWeb, but it should not
own all storage policy. The content provider owns publish/fetch/status/repair
semantics and availability receipts. Carrier owns secure peer discovery,
messaging, relay, and peer/object transport. IPLD gives the traversable CID graph
shape. In the target protected-content cutover, rights, custody, and decryption
remain in Runtime-selected providers.

Provider-to-provider Carrier invocation follows the same boundary. Runtime adds
an `elastos.provider.invocation/v1` envelope and selects
`carrier-provider-plane`. The current Carrier provider target allowlist includes
`content`, `availability`, and the provisional `rights`, `key`, `decrypt`, and
`drm` labels. It does not yet include a custody route. The target cutover keeps
the same transport envelope but replaces the provisional protected-content
surface with Runtime-selected `rights`, `custody`, and `decrypt` providers. Raw
connect tickets stay inside Runtime transport state and are not returned in
app-visible receipts. Raw backend providers such as `ipfs` or `localhost` remain
local implementation details, not remote Carrier authorities.
If the provider transfer is `stream`, the Carrier side also validates the
target-visible `elastos.provider.stream/v1` base64-chunk contract before
dispatching the request. Carrier availability fetches use that path for remote
`content/fetch` calls and decode the returned stream envelope inside the
availability provider.

See [CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md).

### Where HTTP Fits

HTTP is not Carrier. In this repo it plays three supporting roles:

1. **Node-local control API**
   - `elastos-server` exposes an HTTP API for capability requests, provider dispatch, session handling, and orchestration.
   - This is runtime control-plane traffic, not Carrier semantics.

2. **Browser / gateway compatibility**
   - HTTPS gateway URLs are convenience access paths for browsers and installers.
   - Trust should still come from hashes, signatures, and trusted DIDs, not from HTTP itself.

3. **Tunnel / edge bridging**
   - `tunnel-provider` and similar components can expose services over HTTP(S) to the public web.
   - That is an interoperability edge, not the definition of Carrier.

### Where Browser Networking Fits

A real browser capsule should not receive raw internet authority. The browser
engine adapter may use local IPC, vsock, stdio, or loopback to talk to the
Runtime, but the capsule-facing contract remains the typed Runtime
Browser/Net/Exit resource model:

```text
browser capsule -> elastos://net/* -> Runtime Net provider -> Carrier/Exit provider
```

The first `net-provider` implementation is deliberately not an exit provider.
It validates Browser requests, blocks LAN/private targets by default, and
returns an explicit Exit handoff instead of touching host networking itself.
The first `exit-provider` implementation defines that internal egress contract
and also fails closed; it is a boundary for future local, remote Carrier,
privacy, paid, or enterprise exit backends, not a direct host-network escape.
The first constrained `http_fetch` backend is operator-configured through
`ELASTOS_EXIT_PROVIDER_CONFIG`, host allowlists, body limits, and private-target
blocking by default. Browser capsules call the Runtime-owned Browser open route;
Runtime performs the internal handoff from `elastos://net/*` to
`elastos://exit/*` only after Net validation.
The first `stream_relay` backend proof reserves typed stream-session receipts.
Configured stream backends can return two private Unix-socket descriptors:
`elastos.adapter-ipc/v1` for the engine-side bridge and
`elastos.exit.relay-ipc/v1` for an operator/Carrier Exit daemon. Runtime owns
the socket between them.
Remote Carrier exits are the cross-runtime form of the same internal Exit
handoff. They are configured on `exit-provider` as `remote_carrier_exits` with a
remote Runtime DID/service, private `connect_ticket`, stable operator
`grant_id`, allowlisted principals, allowlisted public hosts/schemes/ports, an
optional `expires_at` Unix timestamp, a global active-stream quota, and an
optional per-principal active-stream quota.
Expired grants remain diagnosable as `state=expired` in provider status, but
are excluded from discovery and rejected by `quote` / `open_stream`. The returned
`elastos.exit.remote-carrier.discovery/v1` response is principal-scoped and lists
only permitted Carrier stream exits, with the grant id, policy, and accounting
but no allowed-principal list, host socket, relay socket, or direct TCP
authority. `quote` is also a preview surface and does not expose private route
metadata. The `elastos.exit.remote-carrier-session/v1` receipt carries
`byte_transport=carrier_stream`, the grant id, and a Carrier peer/service
descriptor with the private `connect_ticket` consumed by the Runtime-owned
Carrier stream bridge. Browser capsules and Browser summary responses never
receive the ticket, a host socket, a relay socket, or direct TCP authority.
The local Browser engine still receives only a Runtime-owned Unix stream socket.
Runtime opens the remote byte path with the Carrier `browser_exit_stream`
operation, and the remote runtime must delegate to its own `exit-provider`
`open_stream` policy. Bytes flow only after that remote provider returns a
private `elastos.exit.relay-ipc/v1` descriptor; otherwise the Carrier stream
fails closed.
Before collecting operator evidence, check that the installed source and exit
configs are in the remote-only shape for the reviewed route:

```bash
node scripts/remote-carrier-exit-readiness.mjs \
  --source-config /path/to/source/exit-provider.json \
  --exit-config /path/to/remote/exit-provider.json \
  --principal person:local:alice \
  --grant-id operator-grant:server-exit:alice \
  --target tls://example.com:443 \
  --exit-did did:elastos:server
```

Also check the installed gateway and `exit-provider` artifacts for the Browser
Carrier stream operation and remote Carrier Exit provider contracts. Config
readiness is not enough if the running binaries are stale:

```bash
node scripts/remote-carrier-exit-artifact-readiness.mjs \
  --gateway-bin /path/to/elastos \
  --exit-provider-bin /path/to/exit-provider
```

For public live, prepare the artifact replacement as a dry-run update plan before
touching the server. The plan reuses the same artifact-readiness guard, records
candidate hashes, names the live backup/stage/install/verify/rollback commands,
copies candidates to a server-side staging directory before install, labels
which commands run on the operator workstation versus the public server, rejects
non-Linux or non-`x86_64` candidates such as local macOS Mach-O builds, and
keeps `mutation_allowed=false` until an operator explicitly approves deployment.
For the current public server, candidates must be Linux x86_64 ELF binaries:

```bash
node scripts/remote-carrier-exit-public-live-plan.mjs \
  --candidate-gateway-bin elastos/target/release/elastos \
  --candidate-exit-provider-bin capsules/exit-provider/target/release/exit-provider \
  --installed-artifact-readiness /tmp/elastos-public-live-installed-readiness.json
```

The plan is not acceptance evidence by itself. After approval and restart, rerun
installed artifact readiness against the public-live paths, then collect route
readiness, Browser machine proof, and operator evidence for the reviewed remote
Carrier exit lane.

To create the source-side candidate config from an exit runtime ticket without
printing the private ticket, write the ticket to a local owner-only file, or
save the exit runtime's `elastos.carrier.bootstrap/v1` JSON directly, and
generate a candidate first:

```bash
node scripts/remote-carrier-exit-source-config.mjs \
  --source-config /path/to/source/exit-provider.json \
  --exit-config /path/to/remote/exit-provider.json \
  --exit-ticket-file /path/to/private-exit-ticket.txt-or-bootstrap.json \
  --exit-peer-did did:elastos:server \
  --principal person:local:alice \
  --grant-id operator-grant:server-exit:alice \
  --target tls://example.com:443 \
  --candidate-config /tmp/source-exit-provider.remote.json \
  --receipt-out /tmp/source-exit-provider.remote-receipt.json
```

The generated receipt is redacted and readiness-bound; it records that a ticket
exists and the ticket digest, but not the ticket itself. Only rerun with
`--install` after reviewing the candidate, because installing replaces the
source `exit-provider.json` with the remote-only config and backs up the old
local-exit config beside it. Restart the source runtime/gateway after install
so `exit-provider` reloads the config.

For the product Browser settings path, where the user must choose between a
local exit and a seed-node exit, keep the local backend and add the remote grant
instead of using the remote-only acceptance shape:

```bash
node scripts/remote-carrier-exit-source-config.mjs \
  --source-config /path/to/source/exit-provider.json \
  --exit-config /path/to/remote/exit-provider.json \
  --exit-ticket-file /path/to/private-exit-ticket.txt-or-bootstrap.json \
  --exit-peer-did did:elastos:server \
  --principal person:local:alice \
  --grant-id operator-grant:server-exit:alice \
  --remote-exit-id seed-node \
  --target tls://ela.city:443 \
  --allowed-scheme tcp \
  --allowed-scheme tls \
  --allowed-port 80 \
  --allowed-port 443 \
  --candidate-config /tmp/source-exit-provider.with-seed.json \
  --receipt-out /tmp/source-exit-provider.with-seed-receipt.json \
  --keep-local-backends
```

Review the candidate, then rerun with `--install --keep-local-backends` and
restart the source runtime. Browser settings will show `Local Runtime exit` plus
the remote seed option for principals covered by the grant. This selectable
product mode is not the strict remote-only acceptance lane; use the default
remote-only mode when collecting proof that no local fallback exists.

The readiness report must pass before acceptance evidence is meaningful. It
fails if the Browser source still has a local Exit backend fallback, if the
selected `remote_carrier_exits` grant is missing, expired, not permitted for the
reviewed principal or target, lacks a private route ticket, or points at the
wrong exit DID. It also fails if the remote exit runtime lacks a target-matching
`stream_relay` backend with private `adapter_ipc` and `relay_ipc` descriptors.
The report hash-binds the source and exit config files with `config_sha256`,
records only whether private route material exists, and never prints the
`connect_ticket` or IPC paths from private descriptors.
Before claiming real operator-to-operator Browser Exit acceptance, record
operator evidence with:

```bash
node scripts/remote-carrier-exit-operator-report.mjs --template \
  > /tmp/elastos-remote-carrier-exit-operator-evidence.json
node scripts/remote-carrier-exit-operator-report.mjs \
  --input /tmp/elastos-remote-carrier-exit-operator-evidence.json
```

The report must describe two distinct runtimes with fixed source/exit roles and
distinct endpoint evidence, `carrier_stream` byte transport, the
`browser_exit_stream` operation, remote `exit-provider` relay handoff, redacted
ticket handling, no raw socket/DNS authority to the Browser capsule, target
allowlist enforcement, accounting/quota or close evidence, cleanup, and the
hash-bound redacted artifact references for the local Carrier authority check,
installed artifact readiness report, route-readiness report, source gateway log,
exit gateway log, and Browser machine proof. Each artifact reference carries a redacted local or
remote path plus the SHA-256 digest of the reviewed redacted artifact; a path
without a digest is not acceptance evidence.
The route section must name the reviewed principal, grant id, and target.
Two-runtime evidence must cite the exact source/exit runtime DIDs and endpoint
evidence. Discovery, policy, accounting, quota/close, stream transport, and
cleanup evidence must cite the exact route nouns instead of relying on generic
operator prose.
When the redacted artifact path is available locally, the validator checks the
digest against the file and scans the reviewed artifact for private route
material before accepting the report. A local Browser machine-proof artifact
must also cite the reviewed route target or target host before acceptance;
local installed artifact readiness artifacts must be
`elastos.remote-carrier-exit.artifact-readiness/v1` reports with `ok=true`;
local route-readiness artifacts must be
`elastos.remote-carrier-exit.readiness/v1` reports with `ok=true`, matching
route principal/grant/target, and source/exit `config_sha256` values;
remote-only paths still need the operator-provided digest and review trail.
The validator rejects unreviewed templates and any evidence that includes
private route material such as `connect_ticket`, `relay_ipc`, `adapter_ipc`,
`runtime_stream_path`, or ticket secrets.
The internal `browser-engine-adapter` provider is the matching engine boundary:
it reports adapter status and refuses page launch unless the stream session has
attached `adapter_ipc` byte transport. This keeps CEF/Chromium/WebView work
behind the same typed Runtime Browser/Net/Exit contract instead of exposing host
browser or socket authority to web pages.
Native Linux engine processes go through `browser-engine-supervisor`, a small
host helper that reads `ELASTOS_BROWSER_ENGINE_SUPERVISOR_CONFIG`, starts the
operator-approved engine under `linux_new_netns`, and returns a typed supervisor
proof. It passes only the adapter IPC path, stream id, target, and URL to the
engine; wallet, chain, filesystem, DNS, and direct network authority stay outside
the browser process contract.
The local byte path is separate: `browser-stream-bridge` reads
`ELASTOS_BROWSER_STREAM_BRIDGE_CONFIG`, accepts a private Unix-socket connection
from the engine side, and forwards bytes only to a Runtime-owned Unix stream
socket. It contains no TCP socket, DNS, HTTP client, wallet, chain, or filesystem
authority beyond the configured socket paths.
`browser-engine-supervisor` can launch this bridge before the native engine when
the operator-approved supervisor config includes a `stream_bridge` program and
the internal `adapter_ipc` descriptor carries `runtime_stream_path`. Gateway
allocates that path under `Runtime/BrowserStreams/` after Net/Exit validation and
before Browser Engine launch. Gateway then relays bytes to the private
`relay_ipc` Exit socket when one is configured, or accepts and closes
fail-closed when no Exit relay exists. Browser UI responses never include
`adapter_ipc`, `runtime_stream_path`, or `relay_ipc`, and Browser Engine Adapter
never receives `relay_ipc`.
The first server-side Exit daemon is `browser-local-exit`: it requires
`ELASTOS_BROWSER_LOCAL_EXIT_CONFIG`, accepts typed `elastos.exit.relay-open/v1`
handshakes only from Runtime, dials only operator-allowlisted public targets,
and blocks private resolved IPs unless the operator explicitly enables them.

The visible Browser capsule uses `/api/apps/browser/open` as its product ABI.
That route is still Runtime-owned: it validates the Browser launch grant,
derives the stream target, calls `elastos://net/stream`, performs the internal
`elastos://exit/open_stream` handoff, and only then calls
`elastos://browser-engine/launch`. Ordinary apps never receive a route to
`elastos://exit/*` or `elastos://browser-engine/*`.
If the selected stream backend has an `elastos.adapter-ipc/v1` endpoint, Runtime
passes that descriptor only to the Browser Engine Adapter and strips it from the
Browser UI response.

For general browsing, the browser engine should own TLS while the selected exit
provider relays streams. HTTP-fetch proxying is a narrower compatibility tool,
not the default browser model. Dapp wallet access should go through
`elastos://wallet/*` and Wallet/Inbox approval, not through app-owned
wallet SDK state. See [BROWSER_CAPSULE.md](BROWSER_CAPSULE.md).

### Identity: Device DID (not account identity)

Carrier node identity is a device DID: an Ed25519 key encoded as
`did:key:z6Mk...`. The DID is deterministically derived from the device key via
`SHA-256("elastos-did-v1" || device_key)`. The device_key file stays on disk
for local protection, but peer-visible node identity is the DID.

This is not the same as a human account identity. Home accounts are runtime
principals unlocked by passkeys, and they may later link `did:elastos`, wallet,
or EID proofs. Chat sessions use ephemeral DIDs (random SigningKey per
session). The seed node and `elastos serve` use stable DIDs derived from the
persisted device key.

## Node Planes

### 1. Node Core / Control Plane (host ↔ capsule)

The trusted orchestration layer around Carrier. Manages capsule lifecycle, capability grants, session auth, provider dispatch, and audit.

**Current implementation:**
- `elastos-server` (CLI + HTTP API)
- `elastos-runtime` (capability authority, request handler)
- `elastos-identity`, `elastos-tls`, `elastos-namespace`

**Transport:** a substrate-specific, capsule-scoped Runtime adapter. Components
use ElastOS Bus; web projections and microVMs use their documented narrow
adapters. HTTP, `postMessage`, stdio, vsock, serial, and local IPC are host
plumbing, not Carrier or the capsule contract.

HTTP here is a control-plane protocol. It is not the Carrier substrate.

### 2. Carrier Network + Content Plane (node ↔ world)

Peer discovery, gossip messaging, relay, and peer-to-peer content transfer. Built into the runtime as `carrier.rs`.

**Current implementation:**
- Built-in Carrier node using **Iroh 1.0.2** with iroh-gossip 0.101.0,
  distributed-topic-tracker 0.3.5, mDNS, and relay support
- `tunnel-provider` capsule using **cloudflared** (HTTP tunnel to public internet)

**Transport:** Iroh (QUIC + N0 DNS discovery + relay), with mainline DHT topic
discovery through `distributed-topic-tracker` and mDNS on the local network.
Target: interoperability with Elastos Carrier Native / Boson when those
ecosystems mature.

### 3. Data Plane (host ↔ VM networking)

The physical network plumbing connecting each VM to the host.

**Current implementation:**
- `elastos-crosvm/network.rs`: TAP devices via ioctl, /30 subnets, host-only link (no iptables, no ip_forward)
- TAP is no longer the default for ordinary app capsules; it is used when a capsule explicitly needs guest IP networking or a TCP bridge
- Ordinary Apps use typed Runtime resources through their selected substrate adapter

**Transport:** Linux TAP only when guest networking is explicitly enabled.
Otherwise the substrate adapter communicates with Runtime without granting the
guest ambient network authority.

## Naming in Code

The codebase currently uses "Carrier" in a broader way than this document recommends. Specific usages:

| Term in Code | Meaning | Sub-Plane |
|---|---|---|
| `CarrierNode` | Built-in P2P node (iroh endpoint + gossip) | Network Plane |
| `CarrierGossipProvider` | Provider trait impl for `elastos://peer/*` | Network Plane |
| `start_carrier_node()` | Starts the built-in Carrier node with DID identity | Network Plane |
| "Carrier control link" | legacy name for host↔VM control plumbing (now serial bridge by default, TAP only for explicit guest-network cases) | Data / Control Plane |
| `CarrierServiceBridge` | Host-native provider process (stdio JSON) | Node Core / Control Plane |
| `CapsuleBackend::Carrier` | Capsule runs on host (not in VM) | Node Core / Control Plane |
| `permissions.host_process: true` | Capsule needs Runtime-owned host-process provider execution with host-level network/system access | Control + Network |
| `tunnel-provider` | Public HTTP tunnel via cloudflared | Network Plane |

Recommended reading:

- terms like `CarrierNode` still fit the historical/network meaning
- terms like "Carrier control link" are implementation legacy and should be read as node-control/data-plane plumbing, not as the definition of Carrier itself

## Why Carrier Is Built-In (Not a Capsule)

The ElastOS principle says "everything is a capsule." Carrier is the exception because it does two things:

1. **Bootstrap transport** — file serving, trusted source discovery, and update fetch. These must work BEFORE any capsule infrastructure is available. A capsule can't provide the transport needed to download itself.

2. **Gossip provider** — the `elastos://peer/*` scheme for chat and agent. This shares the same iroh endpoint as the bootstrap transport. Extracting it to a capsule would mean either two iroh endpoints (wasteful) or a shared-endpoint mechanism between runtime and capsule (complex).

The earlier `peer-provider` capsule was the capsule form of this. It was superseded when Carrier was integrated into the runtime for reliability and simplicity, and it is no longer part of the active tree.

**Future:** When the gossip protocol stabilizes, the gossip provider portion of Carrier could be extracted back into a capsule, using the runtime's iroh endpoint via a shared-endpoint API. This is tracked as a later task, not a current priority.

## Open Questions

1. **Inter-capsule communication.** VMs currently cannot talk to each other — only to the host. Carrier should eventually provide capsule-to-capsule channels mediated by the runtime (capability-gated, audited).

2. **Convergence with Carrier Native v2.** The DHT+services model of Carrier v2 maps well to the provider model here. But no integration work has started. Is this a priority?
