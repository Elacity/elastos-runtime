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
│  ├── Providers (did, peer, ai, localhost, ipfs)    │
│  └── Capabilities + Sessions + Audit               │
└────────────────────────────────────────────────────┘
```

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
- providers define the meaning of subspaces such as `elastos://peer/`, `elastos://did/`, and `elastos://ai/`

Clean mental model:

- `elastos://` = namespace / contract surface
- Carrier = decentralized substrate behind peer/content operations
- runtime = trusted node core that hosts Carrier and enforces policy

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

### Identity: DID (not device_key)

All Carrier identity is `did:key` — an Ed25519 key encoded as `did:key:z6Mk...`. The DID is deterministically derived from the device key via `SHA-256("elastos-did-v1" || device_key)`. The device_key file stays on disk for encryption at rest, but external identity is always DID.

Chat sessions use ephemeral DIDs (random SigningKey per session). The seed node and `elastos serve` use stable DIDs derived from the persisted device key.

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

The legacy `peer-provider` capsule was the capsule form of this. It was superseded when Carrier was integrated into the runtime for reliability and simplicity. The capsule is archived at `archive/capsules/peer-provider/`.

**Future:** When the gossip protocol stabilizes, the gossip provider portion of Carrier could be extracted back into a capsule, using the runtime's iroh endpoint via a shared-endpoint API. This is tracked as a later task, not a current priority.

## Open Questions

1. **Inter-capsule communication.** VMs currently cannot talk to each other — only to the host. Carrier should eventually provide capsule-to-capsule channels mediated by the runtime (capability-gated, audited).

2. **Convergence with Carrier Native v2.** The DHT+services model of Carrier v2 maps well to the provider model here. But no integration work has started. Is this a priority?
