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
- **Digital Capsule** = the portable software/data object model

[CAPSULE_MODEL.md](CAPSULE_MODEL.md) expands this terminology, but it is a
supplemental note, not the primary behavior contract.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                         User / Browser                               │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│                    Shell (TUI / CLI / Web shell)                     │
│                    (Capsule with orchestrator capability)            │
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
│  │  │ Shell   │ │localhost│ │ App A   │ │ App B   │            │   │
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
| Server / headless | Linux, any | Runtime proxies capsule HTTP. PC2 is a web dashboard. |
| Desktop | Linux, Windows, macOS | Capsules open in browser tabs or native windows. |
| Mobile | Android, future iOS | Capsules render in embedded webviews. |
| Kiosk | Jetson, appliance | Runtime owns the display. PC2 is the desktop. |

Capsules do not know which host adapter is active. A capsule that serves HTML on
its `http_port` works identically whether the host proxies it to a remote browser,
opens a local tab, renders it in a webview, or displays it fullscreen. The Carrier
bridge, provider access, and capability model are the same everywhere.

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
| **Bootstrap** | Launches shell capsule at startup |

**Size target:** 5-7K lines of Rust (aspirational; currently ~16K across runtime + common crates). TCB reduction via capsule extraction — localhost-provider and did-provider are already separate processes. If it doesn't need to be here, it shouldn't be.

This layer is the **Node Core**, not the Capsule Runtime. It is the trusted host-side enforcement authority. The current repo still carries more host-side orchestration than the end-state architecture described here.

### Layer 2: Shell (Capsule)

A capsule with the **orchestrator capability**. Handles all policy decisions:

| Function | Description |
|----------|-------------|
| **Protocol registry** | Maps registered provider schemes and `elastos://` routes to provider capsules |
| **Permission prompts** | Asks user, then requests capability tokens |
| **Orchestration** | Launches/stops capsules, manages windows |
| **Trust UI** | Shows warnings for untrusted capsules |

**Key insight:** Shell is NOT privileged code. It runs in a sandbox like everything else. It holds the orchestrator capability, which grants it policy authority over other capsules, but it is still sandboxed and subject to runtime enforcement.

## Architecture Decisions

These are the current architectural decisions that matter most when reading the repo:

| Decision | Why |
|----------|-----|
| Runtime stays small and trusted | Isolation, capabilities, signatures, and content trust are the TCB |
| Shell is a capsule, not part of the TCB | Policy can evolve without reclassifying UI code as trusted |
| First-party provider UX converges under `elastos://` | The runtime should expose one native namespace, not a grab bag of unrelated top-level schemes |
| Release trust is signature-based, not gateway-based | Transport can change; trust must come from signed artifacts and trusted publisher identity |
| Carrier is a decentralized substrate, not the whole app contract | Capsules consume namespace/provider contracts, not implementation details like Kubo, QUIC, or cloudflared |

### Layer 3: Capsules

All user software, including providers:

| Type | Examples |
|------|----------|
| **Providers** | localhost://<file-backed roots>/, elastos://did/, elastos://peer/, elastos://ai/ |
| **Applications** | Chat, notepad, photo editor |
| **Utilities** | File manager, settings, terminal |

**Zero ambient authority.** Every action requires a capability token.

Under the broader Digital Capsule model, these are role variants of the same object model:

- app capsules
- provider capsules
- shell capsules
- agent capsules
- data capsules

The per-capsule execution surface that lets these run across WASM and microVM is the **Capsule Runtime** (AppCapsule Runtime), not the trusted node core.

Carrier owns decentralized peer/content semantics. Application capsules consume provider
contracts such as `elastos://peer/`, `elastos://ipfs/`, and `elastos://tunnel/`.
Transport details (QUIC, cloudflared, Kubo, TAP plumbing, local HTTP bridges) are implementation
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
- A human using a shell surface (TUI, CLI, or web shell)
- An AI agent running as a capsule
- An automation script
- A background service

All actors:
- Authenticate with Ed25519 keys
- Request capabilities through the same API
- Receive the same token format
- Are evaluated by the same capability machinery

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
localhost://Users/self/Documents/report.pdf → Local user file
localhost://MyWebSite/index.html            → Local browser-facing site root
localhost://Public/manual.pdf               → Locally shared public file
google://drive/vacation-photos  → Third-party provider example (aspirational, not implemented)
elastos://peer/alice@home/shared/music  → P2P from Alice's device
elastos://ai/claude/chat        → AI provider
```

### Provider + Content Separation

```
google://drive/photo.jpg   (aspirational provider example)
   │          │
   │          └─► Content path (what to fetch)
   └─► Provider (how to fetch, credentials)

Once fetched, content becomes:
   elastos://Qm789xyz (local, provider-independent)
```

This means:
- Content survives provider deletion
- Content can be shared without sharing credentials
- Provider can be swapped without losing data

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
1. Capsule → Shell: "I need to read localhost://Users/self/Pictures/cat.jpg"
2. Shell → User: [Permission prompt]
3. User approves
4. Shell → localhost-provider: fetch("Users/self/Pictures/cat.jpg")
5. Provider returns content (now has CID: elastos://Qm789)
6. Shell → Runtime: grant(capsule, elastos://Qm789, read)
7. Runtime: signs token, emits audit event
8. Runtime → Shell: Token { signed }
9. Shell → Capsule: content + token
10. Capsule can re-read using token (until expiry/revocation)
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
1. Load configuration
   - User identity (Ed25519 keypair)
   - Shell CID (default embedded, configurable)
   - Trusted keys

2. ElastosRuntime::build(config, compute, fetcher)
   - SecureTimeSource::with_persistence()   — monotonic counter (anti-backdating)
   - AuditLog::with_file()                  — security event log
   - MetricsManager::new()                  — rate limiting, telemetry
   - CapabilityStore::with_persistence()    — persisted tokens
   - CapabilityManager::load_or_generate()  — Ed25519 signing key
   - CapsuleManager::new()                  — lifecycle management
   - MessageChannel::new()                  — inter-capsule messaging
   - ContentResolver::new()                 — elastos:// CID resolution
   - RequestHandler::new()                  — protocol processing
   - ShellManager::new()                    — shell bootstrap

3. ElastosRuntime::start()
   - ShellManager::bootstrap()
   - Spawns shell capsule (ELASTOS_API + ELASTOS_TOKEN env vars)
   - Shell takes over user interaction

4. Runtime waits
   - Processes capability requests
   - Manages capsule lifecycle
   - Handles elastos:// fetches
```

---

## Runtime Interface

### Request Protocol (`handler/protocol.rs`)

All capsule↔runtime communication goes through `RequestHandler`. Operations are split by authority:

| Shell-only (orchestrator) | All capsules |
|---------------------------|-------------|
| ListCapsules | Ping, GetRuntimeInfo |
| LaunchCapsule | SendMessage, ReceiveMessages |
| StopCapsule | StorageRead, StorageWrite |
| GrantCapability | FetchContent |
| RevokeCapability | ResourceRequest (provider-routed) |

### For Shell (Orchestrator)

```rust
// Capsule lifecycle
fn launch(cid: ContentId, config: LaunchConfig) -> Result<CapsuleId>;
fn stop(capsule: CapsuleId) -> Result<()>;  // Also clears capsule memory
fn list() -> Vec<CapsuleInfo>;

// Capability management
fn grant(request: CapabilityRequest) -> Result<Token>;
fn revoke(token: TokenId) -> Result<()>;
fn revoke_all_before_epoch() -> Result<u64>;  // Returns new epoch
```

### For All Capsules

```rust
// Use capabilities (with valid token)
fn invoke(token: Token, action: Action) -> Result<Response>;

// Messaging (with messaging token)
fn send(token: Token, to: CapsuleId, message: Bytes) -> Result<()>;
fn recv(token: Token) -> Result<Option<Message>>;

// Time (secure, runtime-controlled)
fn get_secure_time() -> SecureTimestamp;
```

### Built-in

```rust
// Content-addressed fetch (elastos://)
fn fetch(cid: ContentId) -> Result<Content>;
```

### Internal (Not Exposed to Capsules)

```rust
// Audit - runtime calls internally, capsules cannot access
fn emit_audit_event(event: AuditEvent);  // Called at every security point

// Metrics - runtime tracks, used for rate limiting
fn record_metric(capsule: CapsuleId, metric: Metric);

// Memory - runtime manages
fn clear_capsule_memory(capsule: CapsuleId);  // Called on stop()
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

  "type": "wasm",
  "entrypoint": "main.wasm",

  // Other types: "microvm" (full Linux sandbox), "data" (static content with viewer)

  "resources": {
    "memory_mb": 128,
    "cpu_shares": 100
  },

  "permissions": {
    "network": false,
    "storage": ["localhost://Users/self/Pictures/*", "localhost://Users/self/Pictures/Edited/*"],
    "messaging": []
  }
}
```

### Capability Requests

Capsules declare what they MIGHT need. Shell decides what to actually grant:
- User can deny any request
- Shell can restrict scope (photos/* → photos/vacation/*)
- Tokens have expiry (user chooses: once, session, always)

---

## Provider Capsule Interface

Provider capsules handle protocol:// URIs via stdin/stdout JSON protocol:

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
| `elastos://did/` | DID key management, sign/verify |
| `elastos://peer/` | Carrier network plane for peer discovery, gossip, and P2P transport |
| `google://` | OAuth, Google API, caching (aspirational example, not implemented) |
| `elastos://ai/` | Model routing, API keys, response handling |

---

## Shell ↔ Runtime Communication

Shell runs in sandbox but needs to call runtime:

```
Shell → Runtime: { "cmd": "launch", "cid": "Qm123..." }
Runtime → Shell: { "ok": true, "capsule_id": "cap-1" }

Shell → Runtime: { "cmd": "grant", "capsule": "cap-1", "resource": "..." }
Runtime → Shell: { "ok": true, "token": "..." }
```

Provider capsules use the same line-delimited JSON over stdin/stdout or the
equivalent VM/provider bridge. The transport is internal to Carrier.

---

## Offline-First Design

### Core Principle

Capsules don't know about "the internet." Either content exists (by CID) or it doesn't.

### Provider Responsibility

Providers handle network/cache transparently:

```
Request: google://drive/doc.pdf  (aspirational provider example)

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
elastos-project/                        # Mono-repo root
│
├── elastos/                            # Core runtime (→ own repo later)
│   ├── Cargo.toml                      # Workspace: crates/* + core capsules
│   ├── crates/
│   │   ├── elastos-server/             # CLI binary + HTTP API server
│   │   ├── elastos-runtime/            # Core runtime library (the trusted base)
│   │   ├── elastos-common/             # Shared types (CapsuleManifest, ContentId)
│   │   ├── elastos-guest/              # Guest SDK for capsule developers
│   │   ├── elastos-namespace/          # Content-addressed namespace manager
│   │   ├── elastos-identity/           # WebAuthn/Passkey identity
│   │   ├── elastos-tls/                # Self-signed CA + TLS certificates
│   │   ├── elastos-storage/            # Storage providers (local, IPFS, cache)
│   │   ├── elastos-compute/            # Compute provider (WASM sandbox)
│   │   └── elastos-crosvm/              # crosvm microVM provider
│   ├── capsules/                       # Core: ship with runtime
│   │   ├── shell/                      # Capability policy shell (orchestrator)
│   │   ├── localhost-provider/         # rooted localhost file-backed resources
│   └── tools/
│       └── vsock-proxy/               # Guest bridge helper for Carrier control/network provider wiring
│
├── capsules/                           # Non-essential (→ own repos later)
│   ├── chat/                           # P2P chat TUI (mIRC/IRSSI-style)
│   ├── notepad/                        # Capability-aware CLI notepad
│   ├── did-provider/                   # elastos://did/ — DID identity (did:key, Ed25519)
│   ├── ipfs-provider/                  # IPFS operations via managed Kubo daemon
│   ├── ai-provider/                    # elastos://ai/ LLM routing
│   ├── llama-provider/                 # Local llama.cpp inference
│   ├── tunnel-provider/                # elastos://tunnel/ — Carrier network provider for public tunnels
│   ├── agent/                          # AI agent capsule
│   ├── md-viewer/                      # Markdown viewer (data capsule, multi-doc sidebar)
│   ├── gba-emulator/                   # GBA emulator web capsule (mGBA)
│   └── gba-ucity/                      # Data capsule (open-source ROM included)
│
├── scripts/                            # Dev convenience scripts
│   ├── chat.sh                         # P2P chat launcher
│   ├── notepad.sh                      # Notepad demo launcher
│   ├── gba.sh                          # GBA emulator launcher
│   └── share-demo.sh                   # Content sharing demo
│
├── docs/, ROADMAP.md, TASKS.md, ...
└── archive/
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
2. **Shell as capsule** - Policy decisions in replaceable shell
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
