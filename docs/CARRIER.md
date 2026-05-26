# Elastos Carrier in This Runtime

> Supplemental terminology note.
>
> Read [OVERVIEW.md](OVERVIEW.md) and [ARCHITECTURE.md](ARCHITECTURE.md) first.
> This file narrows the Carrier concept and its placement in the runtime. It is
> not the current shipped-behavior contract. For current behavior and proof
> levels, use [../state.md](../state.md), [COMMAND_MATRIX.md](COMMAND_MATRIX.md), and
> [RUNTIME_REPO_USER_STORY_CHECKLIST.md](RUNTIME_REPO_USER_STORY_CHECKLIST.md).

## What Carrier Is

**Carrier is the decentralized communication and content substrate of an ElastOS node.** It handles peer discovery, messaging, relay, and peer-to-peer content transfer for `elastos://` operations.

Carrier is not the whole runtime. The runtime hosts Carrier and enforces capabilities, sessions, routing, and lifecycle around it. Carrier is also not a specific protocol implementation. The transport underneath (iroh today, Carrier Native/Boson tomorrow) is an implementation detail.

## Capsule Model

From a capsule's perspective, Carrier just works. A capsule calls `peer/gossip_send` and messages appear on every subscribed node. The capsule doesn't know or care whether it's running as native, WASM, or microVM, or whether it's on a Jetson, WSL, or a laptop.

The intended end-state is stronger than "Carrier for remote messages." Capsules
should see one Carrier-style capability plane for local and remote effects. A
same-runtime call may be routed in-process, over stdio, over a browser adapter,
or through loopback HTTP, but that is adapter plumbing below the capsule kernel.
The capsule contract is still Carrier-shaped: signed capability envelope,
target object/service, action, payload, response, subscription, and audit.

```
┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│  chat (TUI)  │  │ chat-wasm    │  │  agent       │
│  (ratatui)   │  │ (ansi_ui)    │  │  (headless)  │
└──────┬───────┘  └──────┬───────┘  └──────┬───────┘
       │                 │                 │
       └────────┬────────┴────────┬────────┘
                │  elastos-guest  │
                │  same interface │
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

**Same machine:** All capsules share one runtime, one Carrier node, one gossip buffer. Messages between native chat and WASM chat on the same machine go through the shared buffer — instant, no network needed.

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
shape. Rights/decryption remain in the access provider and dDRM layer.

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
runtime, but the capsule-facing contract is still Carrier-shaped:

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
The internal `browser-engine-adapter` provider is the matching engine boundary:
it reports adapter status and refuses page launch unless the stream session has
attached `adapter_ipc` byte transport. This keeps CEF/Chromium/WebView work
behind the same Carrier-shaped runtime contract instead of exposing host browser
or socket authority to web pages.
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

**Transport:** serial Carrier bridge for ordinary VM app capsules, HTTP over private guest network only for capsules that explicitly need guest IP bridging, and stdio JSON for host-native services.

HTTP here is a control-plane protocol. It is not the Carrier substrate.

### 2. Carrier Network + Content Plane (node ↔ world)

Peer discovery, gossip messaging, relay, and peer-to-peer content transfer. Built into the runtime as `carrier.rs`.

**Current implementation:**
- Built-in Carrier node using **iroh** (QUIC, gossip, mDNS, relay)
- `tunnel-provider` capsule using **cloudflared** (HTTP tunnel to public internet)

**Transport:** iroh (QUIC + pkarr + relay). Target: interoperability with Elastos Carrier Native / Boson when those ecosystems mature.

### 3. Data Plane (host ↔ VM networking)

The physical network plumbing connecting each VM to the host.

**Current implementation:**
- `elastos-crosvm/network.rs`: TAP devices via ioctl, /30 subnets, host-only link (no iptables, no ip_forward)
- TAP is no longer the default for ordinary app capsules; it is used when a capsule explicitly needs guest IP networking or a TCP bridge
- Carrier serial bridge is the preferred control path for regular app capsules

**Transport:** Linux TAP (host-only) when guest networking is explicitly enabled. Otherwise, app capsules use the serial Carrier bridge and avoid guest networking entirely.

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
| `permissions.carrier: true` | Capsule needs host-level network access | Control + Network |
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
