# ElastOS Architecture

> Architecture direction and trusted-core model, not the canonical shipped-behavior contract.

Use this document for the design shape of the system. For current behavior, proof level, and command/runtime expectations, see [../state.md](../state.md), [COMMAND_MATRIX.md](COMMAND_MATRIX.md), and [RUNTIME_REPO_USER_STORY_CHECKLIST.md](RUNTIME_REPO_USER_STORY_CHECKLIST.md).

## Architecture Direction

ElastOS provides:
- **Security by default** - All code runs in sandboxes with zero ambient authority
- **Content addressing** - Code and data identified by hash, not location (MITM-proof)
- **Capability-based access** - Explicit tokens for every resource access
- **Actor equality** - Humans and AI agents use the same capability system and APIs; authority depends on assigned role, not whether the actor is human or AI
- **Offline-first** - Capsules don't know about "the internet"
- **Simple enough for kids** - No manual configuration needed (design target)

**Core principle:** The runtime should become minimal and timeless. Everything else should move outward into capsules, providers, or explicit operator-managed services.

This document uses the following terminology:

- **Node Core / Runtime** = the trusted node-level control plane
- **Carrier** = the decentralized peer/content substrate
- **Capsule Runtime** = the per-capsule execution contract
- **Digital Capsule** = the portable signed package model for software or sealed content distribution

[CAPSULE_MODEL.md](CAPSULE_MODEL.md) expands this terminology, but it is a
supplemental note, not the primary behavior contract.

## ElastOS Four Quadrants

The four quadrants are the planning frame for balancing the ElastOS World
Computer model. They are not four products and not four independent trusted
cores. They describe which responsibility must be handled by which part of the
system so the runtime does not become a protocol pile and apps do not regain
ambient internet authority.

| Quadrant | Responsibility | In this repo | Must not become |
|----------|----------------|--------------|-----------------|
| **PC2 / Home** | Human front door, object browser, spaces, people, app install/launch UX | Home, System, Inbox, Library, Documents, browser host adapters | trusted-core policy logic or protocol implementation |
| **Runtime** | Isolation, verification, principals, sessions, capabilities, object routing, audit | `elastos` node core, capsule launch, provider routing, Home authority checks | app business logic, social-network bridge, wallet app, or storage backend |
| **Carrier** | Authenticated object/message/stream plane, discovery, sync, replication, content delivery | Carrier abstraction and provider-facing transport contracts | chat-only transport, raw gossip exposed to apps, or replacement for capabilities |
| **Blockchain** | DID/EID, wallet signing, provenance anchors, publisher identity, receipts/licensing hooks | Runtime-facing provider boundary for identity/provenance operations | app database, mandatory UX blocker, DeFi-first layer, or runtime business logic |

The capsule-facing contract must be the same whether an effect is local or
remote:

`capsule -> runtime capability -> Carrier/provider plane -> object/service`

That means app, viewer, and content capsules do not branch on "local file",
"IPFS", "Telegram", "browser", or "internet". They request a capability-scoped
operation against an object or service. The runtime authorizes and routes it.
Carrier or a provider performs the effect. Protocol-specific code belongs in
provider capsules or explicit system services, not in ordinary apps and not in
the gateway edge.

For executable capsules, this contract should appear as a capsule-local kernel
or ABI. The capsule boots against that kernel, uses Carrier-style calls such as
invoke/read/write/subscribe, and receives object handles and capability grants.
HTTP routes, browser messages, stdio, loopback, and in-process calls may still be
used by host adapters, but they are below the capsule contract and must not
become product APIs.

The browser follows the same rule. The target browser capsule is not an iframe
and not a normal host tab with ambient internet. It is a Browser Engine Adapter
behind the same capability model: the engine may render real websites, but its
only off-box network path is the Runtime Net provider and selected Carrier/Exit
provider. Dapp wallet access is injected only as a Runtime-mediated wallet
bridge. See [BROWSER_CAPSULE.md](BROWSER_CAPSULE.md).

COMO is tracked as a runtime-framework research input, not an architectural
dependency. Its C++ component model, runtime reflection, MetaClass-style
packaging, Android aarch64 history, and safety/redundancy lessons may inform
typed interface descriptors and generated capsule-kernel glue. It must not
introduce a second authority model, shared vendor address space, or ambient host
access. See [RUNTIME_FRAMEWORK_RESEARCH.md](RUNTIME_FRAMEWORK_RESEARCH.md).

The near-term balancing sequence is:
1. **Passkey-first Runtime authority** to give Home a phishing-resistant default unlock, principal binding, short-lived sessions, scoped capabilities, and human/agent delegation before wallet-first UX.
2. **Content availability and IPLD-compatible manifests** to make published objects sync through the SmartWeb availability network with signed receipts instead of raw CID creation.
3. **Wallet/DID/node proof adapters** to connect Blockchain services through Runtime authority without exposing wallet RPC, node RPC, or private keys to app capsules.
4. **Spaces / network drives** to make Carrier a real object plane and PC2/Home a real object browser.
5. **Capsule publish/install registry** to make signed software identity and install/update trust real before token or NFT mechanics.

Passkey ceremonies require browser user verification. A passkey proof is a
runtime proof binding for a principal, not a replacement for DID, wallet, or
capability semantics.
Browser host adapters may expose passkey ceremonies for Home/System unlock, but
ordinary app capsules should only receive the resulting scoped launch or
capability grants.

Home is passkey-fronted by default. The first passkey created on a runtime is
the admin. Guest creation is disabled until an admin enables it in System. After
that, guests create their own passkey and principal from Home; the admin controls
whether enrollment is open, not the guest's authenticator. Each passkey principal
owns a separate rooted user area such as `localhost://Users/<principal-root>`.
Turning guest creation off blocks new guest roots; it does not revoke existing
guest passkeys.

Guest roots are intended to be private from the runtime operator/admin by
default once principal-root encryption is implemented. Admin authority may manage
runtime policy, revoke local access, and administer availability, but it should
not decrypt a guest root without the guest's explicit sharing/recovery path or a
future threshold/legal policy. A guest should be able to export recovery material
and migrate the encrypted principal root to another ElastOS runtime.

Passkey removal revokes access, not storage. The user root stays on disk under
its principal-derived localhost path. If all passkeys are removed and a new admin
is created, the new admin receives a new principal root; old roots are orphaned
until an explicit recovery/reassignment flow proves authority and records audit.
System now provides the first explicit reassignment path: a verified Recovery Kit
can recover the original principal/root under the active passkey, revoke sessions
for the replacement passkey and any replaced passkey-root binding, reissue
Home/System tokens, restore included built-in Wallet keys, and record signed
audit. This treats the Recovery Kit like an emergency seed phrase: possession of
the kit plus its password is authority to replace a lost still-registered
passkey.

Principal data encryption starts with an explicit root-protection contract.
Current passkeys prove authority to the runtime; they are not raw decryption
keys. ElastOS now creates a random per-principal data key, wraps it to a
Recovery Kit phrase with AES-256-GCM, stores a runtime-encrypted downloadable
archive plus verified protector metadata, can package downloads behind an
optional user password, and verifies imports against the encrypted root
descriptor. Protected roots now use a runtime-owned AES-256-GCM
object envelope for Documents working copies, Home browser state, and
viewer/content storage. The envelope is bound to the principal, localhost root,
data-key ID, and object URI. Protected roots reject plaintext reads instead of
silently migrating or falling back. A lost RP domain or deleted passkey must not
be papered over by a device-global key; if no recovery protector exists, the
encrypted root is intentionally unrecoverable.
Recovery envelopes should be crypto-agile and quantum-conscious: AES-256 or
ChaCha20-Poly1305 for bulk encryption, ML-KEM-768 or stronger for future
public-key wrapping, HQC as a later backup KEM when standardized, ML-DSA plus
optional SLH-DSA for durable signatures, and explicit algorithm metadata on
every envelope. Classical passkeys, EVM/BTC/ELA wallet proofs, and Ed25519 DIDs
are useful proof bindings but not permanent post-quantum recovery roots.

The runtime exposes this through proof-bound recovery status and Recovery Kit
routes. Creating a kit returns the kit once and stores a runtime-encrypted archive
so the active principal can download it again from System. Import verifies the
phrase-wrapped data key and encrypted descriptor before storing protection state.
If a typed DID recovery proof is supplied, import verifies it through
`did-provider` and accepts it only when it matches an existing DID recovery
protector for the recovered root. That preserves the DID protector, but it does
not replace the Recovery Kit until DID-envelope unwrap/rewrap exists.
If the import is explicitly marked as reassignment, the recovered root must be
canonical for the recovered principal. Any previous passkey-root binding for
that recovered root is removed and its sessions are revoked as part of recovery.
Until a principal has matching verified protection, status must report that
protection is not configured, and malformed or cross-principal requests must fail
closed with signed audit.

WebAuthn PRF is a future protector for wrapping the principal data key on the
client side. The PRF result is key material. It must not be serialized into
passkey registration/authentication responses, logged, or posted to runtime auth
routes. The runtime should store only the protector metadata and wrapped data-key
envelope. If a browser, authenticator, or native host cannot provide the PRF
extension, the flow must fail honestly or use another explicit protector such as
Recovery Kit or DID-backed recovery.

Recovery status may report a root as recoverable only when a matching
`PrincipalRootProtectionV1` record exists for the active principal and rooted
localhost user area and at least one protector has been verified. Cross-principal
protection records must not affect another user's status, and malformed matching
records fail closed instead of silently downgrading into an unprotected status.

### Identity, Names, and Object Claims

ElastOS should not collapse passkeys, DIDs, CIDs, and human-readable names into
one identity bucket:

- **Runtime principal**: the local authority subject for sessions and
  capabilities.
- **Passkey**: the default proof that unlocks a human principal.
- **Device DID (`did:key`)**: the self-certifying node/Carrier identity shown in
  System as Device identity.
- **Account DID (`did:elastos` / EID)**: a linked global identity for portable
  profile, credentials, publisher identity, recovery, service endpoints, DAO
  actions, and globally unique names.
- **CID**: one immutable content graph or object revision.
- **IPLD**: the hash-linked object graph that connects CIDs, heads, manifests,
  provenance, rights policy, and availability receipts.

There should be no `did:localhost` shortcut unless ElastOS intentionally defines
and implements a DID method. Local accounts already have runtime principals and
`localhost://Users/<principal-root>` roots. If a user wants to claim `alice`
globally, that claim must go through an EID/DID-chain namespace or equivalent
registry with consensus-backed uniqueness, transfer, expiry, recovery, and
signed ownership. Local display handles can collide; global names cannot.

Capabilities must bind to principal IDs, DIDs, signatures, CIDs, and signed
heads, not to an unverified handle string. A CID does not need its own DID by
default. A mutable object or collection may later have a signed object identity
or object DID that points to successive CID revisions.

WebAuthn relying-party scope is part of the authority contract. Hosted Home
uses the hosted HTTPS origin; localhost development uses loopback HTTP origins;
installed PWAs inherit their install origin. These worlds do not share passkeys
unless a future native/mobile adapter is modeled as an explicit proof adapter.
Malformed or insecure browser origins must fail closed instead of falling back
to the runtime host authority.

Content availability cuts across that sequence. Published objects should be
modeled as IPLD-compatible content graphs where that helps traversal,
provenance, signed heads, and availability receipts. IPLD is the object graph
model; Carrier is the secure coordination/transport substrate; IPFS/Kubo is the
first block backend; the content provider owns publish/fetch/status/repair
policy. See [CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md).

## Object-Oriented Personal OS Model

ElastOS should behave like an object-oriented personal operating system.

That requires three layers to stay distinct:

| Layer | Meaning | Examples |
|------|---------|----------|
| **Objects** | The user's things | documents, songs, photos, games, sites, identities, published revisions |
| **Capsules** | Software roles that operate on objects | app capsules, viewer capsules, provider capsules, shell capsules, content capsules |
| **Spaces** | Namespaces and resolver surfaces where objects and services live | `localhost://...`, `elastos://...`, WebSpaces |

The most important consequence is that not everything should be modeled as a capsule.
Objects are first-class. Capsules open, render, transform, publish, and serve them.
A content capsule can still be a useful packaging and transport form, but it should
not replace the primary object-first user model.

### Public Naming Contract

Public UI should use the human-facing vocabulary:

- `Home`
- `Library`
- `Documents`
- `Directory`
- `Marketplace`
- `Messages`
- `Profile`
- `System`

Internal runtime and developer docs can continue to use:

- shell
- capsule
- provider
- carrier
- namespace

`Apps` is the public term for user-facing interactive capsules. `Capsules` stays as
the technical term for manifests, runtime roles, diagnostics, and developer docs.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                         User / Browser                               │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│                    Home / Orchestrator                               │
│                    (shell-role capsule with orchestration)           │
│                                                                      │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│                         elastos (runtime)                            │
│                                                                      │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │                    Core Functions                            │   │
│  │                                                              │   │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐       │   │
│  │  │Isolation │ │Signatures│ │Capability│ │ elastos  │       │   │
│  │  │          │ │(Ed25519) │ │  Tokens  │ │   ://    │       │   │
│  │  └──────────┘ └──────────┘ └──────────┘ └──────────┘       │   │
│  │                                                              │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                    Running Capsules                           │   │
│  │                                                               │   │
│  │  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐            │   │
│  │  │ Home    │ │localhost│ │ App A   │ │ App B   │            │   │
│  │  │(orchestr)│ │provider │ │         │ │         │            │   │
│  │  └─────────┘ └─────────┘ └─────────┘ └─────────┘            │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
├─────────────────────────────────────────────────────────────────────┤
│                        Host OS (Linux)                               │
└─────────────────────────────────────────────────────────────────────┘
```

Capsules run in isolated sandboxes. The `type` field in `capsule.json` selects
the compute substrate: `microvm` (crosvm, current full-Linux isolation path),
`wasm` (Wasmtime, lightweight), or `data` (static content). The capsule behavior
contract is intended to stay stable across substrates through the Capsule Runtime layer.

### Host Adapters

The diagram above shows Linux as the host OS, but the architecture is designed so
the runtime contract is identical across platforms. What changes is the **host adapter**
— how the runtime presents capsule output to the user:

| Mode | Host | How capsules appear |
|------|------|---------------------|
| Server / headless | Linux, any | Runtime proxies capsule HTTP. Home is a web dashboard. |
| Desktop | Linux, Windows, macOS | Capsules open in browser tabs or native windows. |
| Mobile | Android, future iOS | Capsules render in embedded webviews. |
| Kiosk | Jetson, appliance | Runtime owns the display. Home is the desktop. |

Capsules do not know which host adapter is active. A capsule that serves HTML on
its `http_port` works identically whether the host proxies it to a remote browser,
opens a local tab, renders it in a webview, or displays it fullscreen. The Carrier
bridge, provider access, and capability model are the same everywhere.

The important split is therefore not "web surface" versus "native app". It is:
- launch authority: who is allowed to start a capsule
- session authority: what that caller is allowed to do once connected
- surface adapter: how the capsule UI is rendered

When this document says "web surface", it means a rendering adapter over HTTP/webview.
It is not a separate product identity and it does not define trust or authority by itself.

`chat-room` is the working example. Home launches it locally with orchestrator rights and its web surface keeps using Home-scoped authority for room operations. A public browser reaches the same capsule through the gateway with a browser session and explicit `room.access` capability instead of Home rights.

Browser access is modeled around the browser principal, not around one-off per-app request forms:
- the browser session identifies the remote browser
- capabilities such as `room.access` are granted to that browser session
- Home opens the Inbox app; Inbox reviews pending browser-access requests and approves or denies them
- the same browser session can then reopen the same capsule surface without requesting access again, while a different browser context such as incognito starts with no session and must request access separately

The host member DID is an approval boundary, not delegated identity. A paired browser guest may be admitted by a member runtime, but it does not inherit that member DID or gain member-signed Carrier transport rights.

Home authority is also explicit. Serving `/apps/home/` may show a standard unsigned desktop so a new browser understands where it is, but that desktop is not a user-owned workspace. The Home summary marks `authority.signed_in=false` and omits identity, appearance, browser state, runtime state, and notifications until passkey sign-in. Runtime ensure, Home state writes, and app launch APIs require a Home-scoped capability; app-specific APIs such as Inbox and System require their own app-scoped launch token. Public summaries may expose the standard app/object catalog, but never bearer session tokens or user-owned state. When the same browser has both a native Home room session and a paired browser session, the native Home room session wins so direct Chat Room access in that browser stays aligned with Home.

---

## The Three Layers

### Layer 1: Runtime (`elastos` binary)

The minimal trusted computing base. Does only what MUST be trusted:

| Function | Description |
|----------|-------------|
| **Isolation** | Manages capsule sandboxes (substrate-agnostic: crosvm, WASM, future: containers) |
| **Signatures** | Verifies Ed25519 capsule signatures |
| **Capabilities** | Issues and validates capability tokens |
| **elastos://** | Fetches content-addressed resources |
| **Bootstrap** | Launches the Home/orchestrator capsule at startup |

**Size target:** 5-7K lines of Rust (aspirational; currently ~16K across runtime + common crates). TCB reduction via capsule extraction — localhost-provider and did-provider are already separate processes. If it doesn't need to be here, it shouldn't be.

This layer is the **Node Core**, not the Capsule Runtime. It is the trusted host-side enforcement authority. The current repo still carries more host-side orchestration than the end-state architecture described here.

### Layer 2: Home / Orchestrator Capsule

A shell-role capsule with the **orchestrator capability**. Handles user policy decisions:

| Function | Description |
|----------|-------------|
| **Policy review** | Presents capability and access decisions in human terms |
| **Launch orchestration** | Requests validated app/object launches from the runtime |
| **Orchestration** | Launches/stops capsules, manages windows |
| **Trust UI** | Shows warnings for untrusted capsules |

**Key insight:** a shell-role capsule is NOT privileged code. It runs in a sandbox like everything else. It holds the orchestrator capability, which grants it policy authority over other capsules, but it is still sandboxed and subject to runtime enforcement.

## Architecture Decisions

These are the current architectural decisions that matter most when reading the repo:

| Decision | Why |
|----------|-----|
| Runtime stays small and trusted | Isolation, capabilities, signatures, and content trust are the TCB |
| Shell-role surfaces are capsules, not part of the TCB | Policy can evolve without reclassifying UI code as trusted |
| First-party provider UX converges under `elastos://` | The runtime should expose one native namespace, not a grab bag of unrelated top-level schemes |
| Release trust is signature-based, not gateway-based | Transport can change; trust must come from signed artifacts and trusted publisher identity |
| Carrier is a decentralized substrate, not the whole app contract | Capsules consume namespace/provider contracts, not implementation details like Kubo, QUIC, or cloudflared |

### Layer 3: Capsules

All user software, including providers:

| Type | Examples |
|------|----------|
| **Providers** | localhost://<file-backed roots>/, elastos://did/, elastos://peer/, elastos://ai/ |
| **Applications** | Chat, editor, photo tool |
| **Utilities** | Home, Library, System, terminal |

**Zero ambient authority.** Every action requires a capability token.

Within the capsule packaging/runtime model, these are role variants of the same deployable unit:

- app capsules
- provider capsules
- shell capsules
- agent capsules
- data capsules

The per-capsule execution surface that lets these run across WASM and microVM is the **Capsule Runtime** (AppCapsule Runtime), not the trusted node core.

Carrier owns decentralized peer/content semantics. Application capsules consume product/provider
contracts such as `elastos://peer/` and the current `elastos://content/` surface.
Transport details (QUIC, cloudflared, Kubo, IPFS Cluster, TAP plumbing, local HTTP bridges) are implementation
details of the runtime, Carrier, and providers, not part of the app capsule contract.

## Capsule Network Model

The intended network model is:

- app capsules have no ambient network
- app capsules talk to the runtime over the Capsule Runtime bridge
- Carrier and providers mediate any allowed external communication

In practice that means:

- normal app capsules should launch rootless, with no TAP device and no guest IP requirement
- internet access should be an explicit runtime capability, not a default NIC inside the capsule
- guest networking remains an explicit compatibility/runtime mode for capsules that truly need raw TCP or guest-facing network services

This keeps the abstraction boundary where it belongs: capsules express intent, and the node decides how that intent is fulfilled.

---

## Actor Equality

**Humans and AI agents use the same runtime model.** The runtime does not grant authority based on whether an actor is human or AI:
- A human using Home or another shell-role surface
- An AI agent running as a capsule
- An automation script
- A background service

All actors:
- Prove authority through runtime-managed proof bindings such as passkeys, DID keys, wallet proofs, or delegated agent keys
- Request capabilities through the same API
- Receive the same token format
- Are evaluated by the same capability machinery

Interaction equality is part of the same rule. If a human can click a button,
an authorized agent should be able to request the same capability-scoped
operation through the runtime/provider plane. If an agent can invoke an action,
the human surface should expose understandable state, labels, and review points
for that action. Browser routes, DOM presence, and iframe placement are not
authority.

Role still matters:

- shell sessions have orchestrator authority
- capsule sessions do not

That is a policy-role distinction, not a human-versus-AI distinction.

---

## WebSpaces: Protocol-Based Addressing

`elastos://` is the native namespace exposed by the runtime. It is not identical to Carrier:

- Carrier backs decentralized peer/content parts of the namespace
- providers define the semantics of subspaces
- the runtime enforces capability checks and dispatch

HTTP sits beside this model, not above it:

- node-local HTTP API for runtime control and orchestration
- HTTPS gateways for browser compatibility
- tunnel exposure for interoperability with the web

Those are access paths and bridge protocols. They do not define trust; hashes, signatures, and capabilities do.

### URI Format

```
protocol://path/to/resource

elastos://Qm123abc              → Content-addressed (built-in)
localhost://ElastOS/Documents/<doc-did>   → Mutable document object
localhost://Users/<principal-root>/Documents/report.pdf → Principal-owned local file
localhost://MyWebSite/index.html            → Local browser-facing site root
localhost://Public/manual.pdf               → Locally shared public file
localhost://WebSpaces/Google Drive/Project X/file.pdf → Mounted third-party WebSpace view (aspirational)
google://drive/files/<stable-file-id>        → Third-party provider target (aspirational, provider-private)
elastos://peer/did:key:z6Mk.../shared/music → P2P from a verified peer
elastos://ai/claude/chat                    → AI provider
```

### Mounted WebSpace + Provider + Content Separation

```
localhost://WebSpaces/Google Drive/Project X/photo.jpg
   │
   └─► Local mounted WebSpace handle shown to Library/Home

google://drive/files/<stable-file-id>   (aspirational provider target)
   │
   └─► Provider-private target: credentials, API, sync, and rate limits

Once fetched, content becomes:
   elastos://Qm789xyz (local, provider-independent)
```

This means:
- Content survives provider deletion
- Content can be shared without sharing credentials
- Provider can be swapped without losing data
- Apps speak mounted WebSpace/provider intent, not raw cloud API authority
- Mutable WebSpace mounts/forks may materialize local provider-owned objects
  and dirty heads, but remote resolver sync, cloud-provider traversal, and
  Carrier availability remain provider/Carrier responsibilities rather than app
  filesystem authority

### Content Availability And IPLD

The product-level content contract is `elastos://content/*`, not raw
`elastos://ipfs/*`.

`elastos://content/*` is where the runtime asks for publish, fetch, status,
ensure, repair, and unpublish. The provider behind that contract can use
`ipfs-provider`, Kubo, Elacity, IPFS Cluster-like replication, volunteer nodes,
or future paid storage networks. Normal app/viewer/content capsules should not
know which backend won.

IPLD belongs inside this content plane as the shape of published object graphs:

- object manifests
- signed channel heads
- provenance records
- availability receipts
- sealed-content descriptors
- release/package indexes

This makes CID-linked SmartWeb objects traversable without turning IPLD into a
network, a storage guarantee, or an access-control system.

Clean publication flow:

```
capsule -> runtime capability -> elastos://content/publish
        -> IPLD-compatible manifest
        -> local Kubo pin through ipfs-provider
        -> Elacity/supernode/volunteer replication
        -> signed availability receipt
        -> elastos:// object/CID link
```

See [CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md) for the detailed contract.

## Trusted And Encrypted Content

ElastOS should assume that many installable capsules and published objects are both
signed and encrypted.

The trust and access model should therefore be:

- CID, DID, hash, and signature prove what the capsule or object is
- encryption protects content at rest, in transit, and in shared storage
- sealed objects carry algorithm metadata so encryption, signatures, KEMs, and
  share schemes can evolve without changing the object model
- capability and policy decide who may decrypt or execute it

The right architecture is not to embed custom license logic inside every app capsule.
Instead, ElastOS should expose an explicit access/decryption provider that behaves
like a rights gate:

- the capsule or object is resolved by stable identity
- the runtime verifies trust material
- an access/decryption provider evaluates ownership, subscription, sharing policy,
  or other rights against the caller's DID and granted capabilities
- authorized callers receive a short-lived decryption capability, plaintext stream,
  or derived working key for that session only

This is a Runtime-native protected-content policy gate:

- the capsule talks to the provider plane, not to a bespoke third-party license SDK
- the provider hides whether the sealed bytes live locally, in Carrier-backed storage,
  or behind another trusted source
- Carrier transports encrypted blobs and remote updates; it does not replace policy
  evaluation or capability checks

That keeps the model clean:

- objects remain object-first
- capsules remain untrusted application roles
- providers handle trust, policy, and decryption
- transport remains an implementation detail under the provider plane

---

## Capability System

### Capability Token

This is an architectural shape sketch, not a claim that every field below is the
exact current shipped struct layout.

```rust
// All fields are pub(crate) — external access through read-only getters only.
// Only sign() can set the signature. Only CapabilityManager::grant() calls sign().
struct CapabilityToken {
    version: u8,              // Format version (extensibility)
    id: TokenId,              // Unique identifier
    capsule: String,          // Who can use this token
    issuer: [u8; 32],         // Runtime's Ed25519 pubkey
    resource: ResourceId,     // What resource (elastos://Qm123)
    action: Action,           // read, write, execute, message, delete, admin
    constraints: TokenConstraints,  // epoch, delegatable, classification, max_uses
    issued_at: SecureTimestamp,     // When created
    expiry: Option<SecureTimestamp>,// When expires (None = until revoked)
    signature: [u8; 64],      // Ed25519 over all above (length-prefixed hash)
}
```

**Delegation:** Depth-1 only. A delegated token inherits the parent's action and constraints but is not itself delegatable. Parent must pass full validation (signature, expiry, revocation) before delegation succeeds. Scope can only be narrowed, never widened.

### Flow

```
1. Capsule → Runtime: "I need to read localhost://Users/<principal-root>/Pictures/cat.jpg"
2. Runtime → owning review surface: Wallet/Inbox for wallet authority,
   System/Inbox for runtime policy, or Inbox for generic capability review
3. User or authorized agent approves
4. Runtime → localhost-provider: scoped fetch through provider registry
5. Provider returns content or object metadata
6. Runtime grants the requesting capsule a scoped read token
7. Runtime signs the token and emits audit events
8. Runtime → Capsule: approved response + token
9. Capsule can repeat the approved action until expiry, revocation, or use limit
```

### Token Validation (Runtime Enforced)

For every capability invocation, 12 checks in sequence (`capability/manager.rs:validate()`):

1. **Version check** — Token format version matches `CURRENT_VERSION`
2. **Signature verification** — Ed25519 signature valid against runtime's verifying key
3. **Issuer verification** — Token issuer matches runtime's public key
4. **Caller verification** — Token's capsule ID matches the requesting capsule
5. **Action verification** — Token's action matches the requested action
6. **Resource verification** — Requested resource matches token's resource pattern (with wildcard support)
7. **Epoch verification** — Token's epoch ≥ global epoch (mass revocation check)
8. **Individual revocation** — Token ID not in revocation set
9. **Future-dated check** — `issued_at` not in the future (anti-backdating)
10. **Expiry check** — Token not expired (if expiry is set)
11. **Use-count check** — Atomic check-and-increment if `max_uses` is set (no TOCTOU)
12. **Classification check** — Token's `max_classification` ≥ resource's classification level

All fields are signed. Hash uses length-prefixed variable-length fields and explicit `Option` discriminants to prevent collision. Token fields are `pub(crate)` with read-only public accessors — immutable after `sign()`.

**Without valid token = action denied. Every check emits an audit event.**

---

## Security Model

### Trust via Content Addressing

```
Capsule request: elastos://Qm123abc

1. Fetch content from any source
2. Compute: actual_hash = SHA256(content)
3. Verify: actual_hash == Qm123abc
4. Verify: signature valid for trusted key
5. Only then: load into sandbox

MITM impossible - content is self-authenticating
```

### Trust Levels

| Level | Description | Behavior |
|-------|-------------|----------|
| **Root trusted** | Signed by foundation key | Full capability requests |
| **Community** | Signed by known developer | Normal access, verified |
| **Untrusted** | Unknown signer | Warnings, restricted defaults |

### Defense in Depth

| Layer | Protection |
|-------|------------|
| Content hash | Tampering detection |
| Signature | Origin verification |
| Sandbox | Memory isolation |
| Capabilities | Access control |
| Encryption | Data at rest |

---

## Boot Sequence

```
1. Load local runtime state
   - data directory
   - device DID / Ed25519 identity
   - trusted source and installed component metadata
   - persisted capability/session state where applicable

2. Start the trusted node core
   - builds the provider registry
   - registers built-in Carrier peer transport
   - registers first-party providers such as localhost, did, webspace, documents,
     and optional chain/content-backend/tunnel/AI providers when installed
   - starts the local API/gateway routes for Home and app-scoped capabilities

3. Serve Home
   - `/apps/home/` is the browser-hosted adapter for the `home` capsule
   - Home receives a Home-scoped capability for its browser context
   - app launches mint app-scoped launch tokens; child apps do not inherit Home authority
   - runtime-backed app launches pass a signed app-scoped `launch_grant` into
     `/api/capsules`; raw `principal_id` injection is rejected for principal
     launches
   - shell/supervisor microVM launches use the same signed grant shape before a
     principal enters `BridgeContext`; provider-role launches cannot receive
     user scope

4. Runtime waits
   - validates capability-scoped requests
   - launches/stops capsules through explicit runtime contracts
   - routes object/provider effects through Carrier or registered providers
```

---

## Runtime Interface

The current implementation exposes this contract through several concrete
bridges: HTTP API routes for Home and browser-hosted apps, provider registry
calls for host-side providers, stdio/serial bridges for some capsule classes,
and explicit CLI commands for operator workflows. Do not treat the sketches
below as one generated API file. They describe the intended authority split that
current and future bridges must preserve.

Operations are split by authority:

| Orchestrator-role surfaces | Ordinary capsules |
|---------------------------|-------------------|
| list launchable objects/apps | request runtime info |
| launch/focus/close capsules | send provider requests with a valid token |
| mint app-scoped launch grants | read/write scoped localhost objects |
| approve/deny capability prompts | fetch scoped `elastos://` / provider objects |
| revoke or expire grants | send/receive authorized messages |

### For Orchestrator-Role Capsules

```rust
// Conceptual lifecycle operations.
fn launch(cid: ContentId, config: LaunchConfig) -> Result<CapsuleId>;
fn stop(capsule: CapsuleId) -> Result<()>;
fn list() -> Vec<CapsuleInfo>;

// Conceptual capability management.
fn grant(request: CapabilityRequest) -> Result<Token>;
fn revoke(token: TokenId) -> Result<()>;
```

### For All Capsules

```rust
// Conceptual provider invocation with a valid token.
fn invoke(token: Token, action: Action) -> Result<Response>;

// Messaging (with messaging token)
fn send(token: Token, to: CapsuleId, message: Bytes) -> Result<()>;
fn recv(token: Token) -> Result<Option<Message>>;

// Runtime-controlled facts should be requested, not guessed.
fn get_secure_time() -> SecureTimestamp;
```

### Built-in

```rust
// Conceptual content/object fetch.
fn fetch(cid: ContentId) -> Result<Content>;
```

### Internal (Not Exposed to Capsules)

```rust
// Runtime-owned audit; capsules cannot emit trusted audit facts directly.
fn emit_audit_event(event: AuditEvent);

// Metrics - runtime tracks, used for rate limiting
fn record_metric(capsule: CapsuleId, metric: Metric);

// Lifecycle cleanup remains runtime-owned.
fn clear_capsule_memory(capsule: CapsuleId);
```

---

## Capsule Manifest

```json
{
  "schema": "elastos.capsule/v1",
  "version": "0.1.0",
  "name": "photo-editor",
  "description": "Simple photo editor",
  "author": "developer-key-id",
  "signature": "base64-ed25519-signature",
  "role": "app",

  "type": "wasm",
  "entrypoint": "main.wasm",

  // Other types: "microvm" (full Linux sandbox), "data" (static content with viewer)

  "resources": {
    "memory_mb": 128,
    "cpu_shares": 100
  },

  "permissions": {
    "network": false,
    "storage": ["localhost://Users/<principal-root>/Pictures/*", "localhost://Users/<principal-root>/Pictures/Edited/*"],
    "messaging": []
  }
}
```

### Capability Requests

Capsules declare what they might need. The orchestrator-policy surface decides what to actually grant:
- User can deny any request
- the grant can restrict scope (photos/* -> photos/vacation/*)
- Tokens have expiry (user chooses: once, session, always)

---

## Provider Capsule Interface

Provider capsules handle provider-scoped requests behind the runtime/provider
registry. Some providers currently use line-delimited JSON over stdio or a VM
bridge; browser-hosted apps and Home reach providers through token-gated runtime
HTTP routes. The capsule-facing rule is the same: provider-specific behavior is
behind the runtime boundary.

```rust
trait Provider {
    fn fetch(&self, path: &str) -> Result<Content>;
    fn store(&self, path: &str, content: &[u8]) -> Result<ContentId>;
    fn list(&self, path: &str) -> Result<Vec<Entry>>;
    fn delete(&self, path: &str) -> Result<()>;
}
```

### Provider Examples

| Provider | Responsibilities |
|----------|-----------------|
| `localhost://<file-backed roots>/` | Encrypt/decrypt, rooted local filesystem access |
| `elastos://did/` | Device DID resolution, typed DID signatures/verification, and fail-closed recovery-proof checks |
| `elastos://peer/` | Carrier network plane for peer discovery, gossip, and P2P transport |
| `elastos://content/` | SmartWeb publish/fetch/status/repair contract above IPFS/Kubo/Elacity/supernode backends |
| `elastos://availability/` | Internal availability-provider seam for configured replication targets; not normal app contract |
| `elastos://ipfs/` | Low-level system/provider backend for local Kubo operations; not normal app contract |
| `elastos://chain/` | Typed chain reads/proofs through `chain-provider`; no raw RPC URLs or node ports |
| `elastos://net/` | Browser/Net requests through `net-provider`; no raw sockets, DNS, host internet, or LAN/private targets without explicit Exit Provider policy |
| `elastos://exit/` | Internal Browser egress contract through `exit-provider`; not normal app contract, not app-visible authority, and no direct host networking without configured backend |
| `elastos://browser-engine/` | Internal Browser Engine Adapter contract; not app-visible authority and no page launch without attached Runtime stream byte transport |
| `elastos://wallet/` | Wallet proof, account-link, approval, typed-signing, and transaction requests through `wallet-provider`; no raw wallet RPC or private-key access |
| `elastos://drm/` | Protected-content open contract through `drm-provider`; no raw CEKs, key-backend SDKs, wallet RPC, chain RPC, Kubo/IPFS APIs, or Elacity credentials |
| `elastos://rights/` | Typed protected-content rights questions through `rights-provider`; no contract SDKs, chain RPC, wallet RPC, or raw key authority |
| `elastos://key/` | Protected-content key-release requests through `key-provider`; no raw CEKs, KMS credentials, chain RPC, or wallet RPC |

The visible Browser capsule uses `/api/apps/browser/open` as its product route.
That route validates the Browser launch grant, reserves a Runtime Net/Exit
stream, and calls the internal Browser Engine Adapter. When an operator
configures the Playwright proof helper, Browser UI receives only a page id plus
Runtime screenshot/input routes for the rendered page. Ordinary apps do not get
raw `elastos://exit/*` or `elastos://browser-engine/*` provider access.
Raw `elastos.adapter-ipc/v1` endpoint descriptors are internal handoff data and
are stripped from Browser UI responses.
Native browser engines are also behind a supervisor proof contract:
`elastos.browser.engine.launch-request/v1` is sent through
`ELASTOS_BROWSER_ENGINE_REQUEST`, and Runtime accepts only a validated
`elastos.browser.engine.supervisor-result/v1` that preserves runtime-net-only,
no-direct-network, and no-wallet-injection invariants.
The matching `browser-stream-bridge` helper is the first local byte-transport
piece: it accepts one private `elastos.adapter-ipc/v1` Unix-socket connection
from the engine side and forwards bytes to a Runtime-owned Unix stream socket.
It does not open TCP sockets, perform DNS, or contact the host internet. Gateway
owns the Runtime stream socket and relays it only to a private
`elastos.exit.relay-ipc/v1` Unix socket returned by the Exit provider; without
that relay, the socket closes fail-closed. Runtime stream sockets use the short
host temp directory `elastos-browser-streams/` to avoid Unix socket path-length
failures while keeping the descriptors private. Browser UI responses never
expose `adapter_ipc` or `relay_ipc`.

The first renderable Browser proof is
`elastos/tools/browser-playwright-engine`. It launches Playwright Chromium
through the Browser Engine Adapter contract, routes page requests through the
configured Exit relay, and exposes a constrained EIP-1193 account/chain bridge
whose signing operations fail closed until the wallet approval path is wired.
The first server-side Exit relay is `browser-local-exit`, a typed Unix-socket
daemon that dials only operator-allowlisted public TCP/TLS targets after Runtime
has validated the Browser stream request. It is the current Browser path's only
DNS/TCP dialer; Browser UI, Browser Engine Adapter, and stream bridge still have
no direct host network authority.
| `elastos://decrypt/` | Protected-content decrypt/render sessions through `decrypt-provider`; no raw CEKs, raw plaintext, filesystem authority, key-backend SDKs, KMS credentials, chain RPC, or wallet RPC |
| `google://` | OAuth, Google API, caching (aspirational example, not implemented) |
| `elastos://ai/` | Model routing, API keys, response handling |

New provider families should document their typed contract before becoming a
Home-visible product surface. For the first blockchain provider slice, see
[Chain Provider](CHAIN_PROVIDER.md), [Wallet Provider](WALLET_PROVIDER.md),
[Rights Provider](RIGHTS_PROVIDER.md), [Key Provider](KEY_PROVIDER.md),
[Decrypt Provider](DECRYPT_PROVIDER.md), and
[Protected Content Provider](PROTECTED_CONTENT.md).
For the content availability direction, see [Content Availability and
IPLD](CONTENT_AVAILABILITY.md).

---

## Orchestrator ↔ Runtime Communication

Home and other orchestrator-role surfaces run as capsules, but need to request
runtime actions:

```
Home -> Runtime: { "cmd": "launch", "target": "documents" }
Runtime -> Home: { "ok": true, "route": "/apps/documents/?home_token=..." }

Home -> Runtime: { "cmd": "grant", "capsule": "documents", "resource": "..." }
Runtime -> Home: { "ok": true, "token": "..." }
```

The exact transport can be HTTP, stdio, serial, or another host adapter. The
authority model must not change: the runtime validates the caller and mints a
scoped grant.

---

## Offline-First Design

### Core Principle

Capsules don't know about "the internet." Either content exists (by CID) or it doesn't.
For published objects, the content provider may also return an availability
state such as local-only, syncing, network-available, or repair-needed. The
capsule still sees object state, not Kubo, gateway, relay, or cluster topology.

### Provider Responsibility

Providers handle network/cache transparently:

```
Provider-internal target: google://drive/doc.pdf  (aspirational example)

Online scenario:
  1. Check cache: miss
  2. Fetch from Google API
  3. Cache locally (encrypted)
  4. Return content + CID

Offline scenario:
  1. Check cache: hit
  2. Return cached content

Capsule sees: content or error. Nothing about network state.
```

---

## Project Structure

```
elastos-runtime/                        # Repo root
│
├── elastos/                            # Core runtime (→ own repo later)
│   ├── Cargo.toml                      # Workspace: crates/* + core capsules
│   ├── crates/
│   │   ├── elastos-server/             # CLI binary + HTTP API server
│   │   ├── elastos-runtime/            # Core runtime library (the trusted base)
│   │   ├── elastos-auth/               # Shared proof/session authority primitives
│   │   ├── elastos-common/             # Shared types (CapsuleManifest, ContentId)
│   │   ├── elastos-guest/              # Guest SDK for capsule developers
│   │   ├── elastos-namespace/          # Content-addressed namespace manager
│   │   ├── elastos-identity/           # did:key / Ed25519 identity
│   │   ├── elastos-tls/                # Self-signed CA + TLS certificates
│   │   ├── elastos-storage/            # Storage providers (local, IPFS, cache)
│   │   ├── elastos-compute/            # Compute provider (WASM sandbox)
│   │   └── elastos-crosvm/              # crosvm microVM provider
│   ├── capsules/                       # Core: ship with runtime
│   │   ├── shell/                      # Capability policy shell (orchestrator)
│   │   ├── localhost-provider/         # rooted localhost file-backed resources
│   └── tools/
│       ├── vsock-proxy/               # Guest bridge helper for Carrier control/network provider wiring
│       ├── browser-engine-supervisor/ # Linux native Browser Engine launch supervisor
│       ├── browser-playwright-engine/ # Server/headless Browser Engine screenshot-input proof
│       ├── browser-stream-bridge/     # Linux Browser Engine Unix stream bridge
│       └── browser-local-exit/        # Server-side allowlisted Browser Exit relay
│
├── capsules/                           # First-party and demo capsules
│   ├── home/                           # Home browser shell-role capsule
│   ├── home-cli/                       # Home CLI capsule
│   ├── system/                         # System settings capsule
│   ├── inbox/                          # Inbox capsule for requests and approvals
│   ├── library/                        # Library capsule for object browsing and opening
│   ├── documents/                      # Documents capsule for local markdown objects
│   ├── chat-room/                      # Browser Chat Room app capsule
│   ├── chat-room-ui/                   # Shared Chat Room browser UI assets
│   ├── chat/                           # Native P2P chat capsule
│   ├── chat-wasm/                      # WASM chat capsule variant
│   ├── gba-emulator/                   # GBA emulator web capsule
│   ├── gba-ucity/                      # Data capsule with included ROM
│   ├── agent/                          # AI agent capsule
│   ├── did-provider/                   # elastos://did/ identity provider
│   ├── chain-provider/                 # elastos://chain/ typed chain provider
│   ├── net-provider/                   # elastos://net/ Browser/Net fail-closed boundary
│   ├── exit-provider/                  # elastos://exit/ internal Browser egress contract
│   ├── browser-engine-adapter/         # elastos://browser-engine/ internal engine adapter contract
│   ├── wallet-provider/                # elastos://wallet/ account-link and approval provider
│   ├── drm-provider/                   # elastos://drm/ protected-content boundary
│   ├── rights-provider/                # elastos://rights/ protected-content rights provider
│   ├── key-provider/                   # elastos://key/ protected-content key release provider
│   ├── decrypt-provider/               # elastos://decrypt/ protected-content decrypt/render provider
│   ├── ipfs-provider/                  # IPFS operations via managed Kubo daemon
│   ├── availability-provider/          # elastos://availability/ configured replication adapter
│   ├── ai-provider/                    # elastos://ai/ LLM routing
│   ├── llama-provider/                 # Local llama.cpp inference
│   ├── site-provider/                  # Local site serving provider
│   ├── tunnel-provider/                # elastos://tunnel/ public tunnel provider
│   └── webspace-provider/              # WebSpaces resolver provider
│
├── scripts/                            # Dev convenience scripts
│   ├── chat.sh                         # P2P chat launcher
│   ├── gba.sh                          # GBA emulator launcher
│   ├── home-demo-local.sh              # Local Home demo launcher
│   ├── home-smoke.sh                   # Home browser smoke test
│   └── share-demo.sh                   # Content sharing demo
│
└── docs/, ROADMAP.md, TASKS.md, ...
```

## Crate-to-Layer Mapping

This is the practical mapping for the current repo:

| Layer | Main crates / code |
|-------|---------------------|
| **Runtime / trusted base** | `elastos-runtime`, `elastos-common`, `elastos-tls`, trusted parts of `elastos-server` |
| **Execution substrates** | `elastos-compute`, `elastos-crosvm` |
| **Identity / storage / namespace support** | `elastos-identity`, `elastos-storage`, `elastos-namespace` |
| **CLI / orchestration / release UX** | `elastos-server` |
| **Shell and provider capsules** | `elastos/capsules/*`, top-level `capsules/*` |

This mapping is descriptive of the current codebase. It is not a statement that every crate boundary is final, and it should not be read as proof that every described surface is equally productized.

---

## Summary

ElastOS is built on six foundations:

1. **Minimal runtime** - Only isolation, signatures, capabilities, elastos://
2. **Home as capsule** - Policy decisions in a replaceable shell-role capsule
3. **Capability tokens** - Cryptographic proof of permission
4. **Content addressing** - MITM-proof, offline-first
5. **Actor equality** - Humans and AI use the same capability model; authority comes from assigned role
6. **Provider separation** - Each protocol:// in isolated capsule

That is the direction. The current repo is still converging toward it and still contains compatibility glue, operator ceremony, and host-side orchestration that do not yet fit the clean final picture.

---

## Enterprise Security Considerations

This section is directional. It describes additional enterprise-grade hardening
and compliance concerns, not a claim that all of them are fully implemented now.

For hospital-grade security and IoT infrastructure deployment, additional requirements:

| Requirement | Purpose |
|-------------|---------|
| **Audit logging** | HIPAA compliance, forensics, accountability |
| **Key hierarchy and recovery** | Enterprise key management, prevent data loss |
| **Emergency access (break-glass)** | Healthcare emergency situations |
| **Secure time source** | Prevent token expiry manipulation |
| **Data classification** | Different protection levels for different data |
| **Revocation propagation** | Ensure revoked tokens stop working everywhere |
| **Secure boot chain** | Protect runtime integrity on unattended devices |
| **Rate limiting** | Prevent resource exhaustion attacks |
| **Side-channel mitigations** | Protect against timing/cache attacks |
