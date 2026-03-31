# Digital Capsule Model

> Supplemental terminology note.
>
> Read [OVERVIEW.md](OVERVIEW.md) and [ARCHITECTURE.md](ARCHITECTURE.md) first.
> This file narrows the capsule/runtime/object vocabulary. It is not the current
> shipped-behavior contract. For current behavior and proof levels, use
> [../state.md](../state.md), [COMMAND_MATRIX.md](COMMAND_MATRIX.md), and
> [RUNTIME_REPO_USER_STORY_CHECKLIST.md](RUNTIME_REPO_USER_STORY_CHECKLIST.md).

Supplemental terminology for capsules, Capsule Runtime, Carrier, and the trusted node core.

This document is the reference point for capsule language in this repo. It exists to keep four ideas separate:

- the trusted **Node Core**
- the decentralized **Carrier** substrate
- the per-capsule **Capsule Runtime** (Rong Chen's "AppCapsule Runtime")
- the **Digital Capsule** as the portable object model

## Core Model

- **Digital Capsule**: the umbrella object.
  A signed, portable, capability-governed software or data object with explicit identity, interface, and lifecycle.
- **Node Core / Runtime**: the trusted node-level control plane.
  It enforces capabilities, sessions, routing, audit, signatures, and lifecycle orchestration.
- **Carrier**: the decentralized peer/content substrate hosted by the node.
  It handles peer discovery, gossip, relay, and peer-to-peer content transfer.
- **Capsule Runtime**: the per-capsule execution contract.
  It is the substrate-independent runtime surface that lets one capsule behave consistently across WASM, microVM, and future backends.
- **WebSpace**: the native namespace and syscall-like addressing surface.
  Capsules express intent through `elastos://`, `localhost://`, and related provider-backed schemes.

Short form:

- Node Core = trusted control plane
- Carrier = decentralized communication/content substrate
- Capsule Runtime = per-capsule execution substrate
- Digital Capsule = portable app/service/data object
- WebSpace = native contract surface

## What A Digital Capsule Is

A Digital Capsule is not just "a process" and not just "a file." It has several layers:

1. **Artifact**
   - immutable package or bundle
   - signed
   - content-addressed or otherwise provenance-tracked
   - described by `capsule.json`

2. **Runtime contract**
   - the ABI and execution surface the capsule expects
   - environment variables, bridge channels, syscalls, provider calls, lifecycle conventions
   - implemented by the Capsule Runtime

3. **Instance**
   - one running copy of a capsule
   - bound to a session, capability set, and execution substrate

4. **State**
   - mutable user, app, or shared state
   - kept separate from the immutable capsule artifact

5. **Head / pointer**
   - mutable pointer to the currently trusted or preferred version
   - separate from immutable published versions

This separation is essential. If artifact, runtime, instance, and state are blurred together, capsules become hard to verify, move, share, and reason about.

## Capsule Taxonomy

Digital Capsule is the umbrella term. In this repo, capsules fall into these categories:

- **Executable capsules**
  - **App capsule**: user-facing app such as chat or notepad
  - **Provider capsule**: protocol implementation such as DID, localhost storage, tunnel, or AI
  - **Shell capsule**: orchestrator UI with policy authority via capability grants
  - **Agent capsule**: autonomous app capsule using the same capability model
- **Data capsules**
  - signed or content-addressed content packages with a viewer or handler

The important point is that these are all capsules. They differ by role, not by escaping the model.

## Capsule Runtime (AppCapsule Runtime)

Rong Chen's "AppCapsule Runtime" should be understood as the common execution substrate for executable capsules.

It is:

- not the trusted node core
- not Carrier
- not the app logic itself

It is the layer that makes one capsule portable across substrates.

Responsibilities:

- binary/module loading
- execution pump and ABI glue
- in-capsule runtime interfaces
- bridge channels to the node
- substrate-specific boot conventions
- walk-away independence model for capsule execution

In current repo terms, this concept is implemented across multiple pieces rather than one crate:

- `elastos/crates/elastos-guest`
- `elastos/crates/elastos-compute`
- `elastos/crates/elastos-crosvm`
- the stdio / serial bridge contracts between guest capsules and the node

So "Capsule Runtime" is currently a conceptual layer with several implementations, not a monolithic library.

## First-Principles Rules

These rules keep the model coherent:

1. **Capsules do not own raw topology**
   - apps should not depend on TAP, relay URLs, QUIC, or host IPs
   - they depend on WebSpace/provider contracts

2. **Capsules have no ambient network**
   - the default capsule contract is Carrier-only communication with the node/runtime
   - internet access, local host access, or third-party fetches are granted capabilities, not guest NICs
   - if a capsule needs broader network behavior, that should be explicit in the manifest and enforced by the node

3. **Carrier and Capsule Runtime are orthogonal**
   - Carrier answers: how do peers and content communicate?
   - Capsule Runtime answers: how does a capsule execute consistently?

4. **Node Core remains minimal**
   - policy enforcement and trust anchors stay in the node core
   - app and provider logic stays outside it

5. **Artifact identity stays distinct from mutable state**
   - immutable published capsule
   - mutable local/shared state
   - mutable trusted head or release pointer

6. **Capsule behavior should converge across substrates**
   - WASM and microVM variants may have different wrappers
   - app behavior, wire format, and capability semantics should stay the same

7. **Providers own semantics after the scheme**
   - `elastos://peer/alice/shared` is named data, not a filesystem path
   - provider capsules define how that namespace is interpreted

## Trust Domains

Capsules operate in one of two trust domains:

### User/Application Domain

App capsules, agent capsules, and user-facing provider capsules run in the **user trust domain**:

- Shell-mediated capability approval (pending request → shell grant/deny)
- Full capability token flow with Ed25519-signed tokens
- Subject to shell policy (auto, cli, or agent/rules modes)
- Bridge provides `BridgeContext` with `PendingRequestStore` and `CapabilityManager`

This is the normal path for `elastos serve` + `elastos run`/`elastos chat`.

### Infrastructure/Service Domain

Gateway-launched capsules (ipfs-provider, tunnel-provider) run in the **infrastructure trust domain**:

- Trusted service-plane components launched by the runtime operator
- Not subject to user shell approval — they ARE the service infrastructure
- No `CapabilityManager` or `PendingRequestStore` attached
- If an infrastructure capsule ever requests a capability, the bridge returns a clear `infrastructure_capsule` denial
- Launched via `elastos gateway --public`, not through the user shell

The distinction matters: forcing service-plane infrastructure through user shell approval blurs two different trust relationships. The operator who runs `elastos gateway --public` is explicitly trusting those capsules as part of the node's service layer.

### Why Two Domains

- User capsules are untrusted by default — they must request and be granted capabilities
- Infrastructure capsules are operator-trusted — the operator chose to run them as part of the node
- Collapsing these into one model either over-restricts infrastructure (unnecessary approval prompts) or under-restricts apps (implicit trust that should be explicit)

## Guest Networking

Guest networking is useful, but it is not the default ElastOS model.

The preferred model is:

- capsules get no ambient network
- capsules talk to the node through Carrier and the Capsule Runtime bridge
- the node brokers allowed effects through provider calls and capability grants

That means:

- a normal app capsule should be able to run with no TAP, no guest IP, and no sudo at launch
- internet access should appear as an explicit granted capability such as fetch, tunnel, or provider-mediated access
- host resources should be exposed through provider contracts, not by leaking host topology into the capsule

Guest networking remains useful as an explicit compatibility mode for:

- provider capsules that must expose or consume real guest TCP services
- legacy workloads that assume raw sockets or a conventional Linux network stack
- migration paths while a workload is being adapted to the Carrier/provider model

So the long-term rule is:

- **Carrier-only by default**
- **guest network only when explicitly requested and justified**

## Rong Chen Alignment

Two Rong Chen insights are central here:

1. **URI as named data / syscall surface**
   - WebSpace URIs are Named Data Network representations
   - a capsule emits intent through a URI
   - the node/runtime launches or routes to the corresponding provider

2. **Capsule as the durable application unit**
   - capsules should be portable, self-describing, and independent from one host or cloud account
   - the host OS should not leak into app semantics

This implies:

- apps should speak WebSpace intent, not transport detail
- providers should implement URI semantics
- the Capsule Runtime should make execution portable
- Carrier should stay below app semantics

## Current Repo Mapping

Today the repo maps to this model as follows:

- **Node Core / Runtime**
  - `elastos/crates/elastos-server`
  - `elastos/crates/elastos-runtime`
  - supporting trusted crates such as identity, namespace, and TLS

- **Carrier**
  - built-in node in `elastos/crates/elastos-server/src/carrier.rs`
  - relay, gossip, DHT, peer/content transport behavior

- **Capsule Runtime**
  - `elastos-guest` SDK
  - WASM execution backend in `elastos-compute`
  - microVM execution backend in `elastos-crosvm`
  - bridge protocols used by stdio and serial guest communication

- **Digital Capsules**
  - app capsules in `capsules/`
  - provider capsules in both `elastos/capsules/` and top-level `capsules/`
  - data capsules published through share/content flows

## Current vs Target State

Current state:

- the model is real, but not fully uniform everywhere yet
- the chat milestone is the clearest proof of direction: one shared core, multiple artifacts, shared wire format

Target state:

- one capsule behavior model across WASM and microVM
- providers fully accessed through WebSpace/provider contracts
- clean separation of Node Core, Carrier, Capsule Runtime, and Digital Capsule terms

### Substrate-Specific UI Surfaces

Substrates differ in what host capabilities they expose to capsules:

- **MicroVM** — full Linux environment: raw terminal mode, alternate screen, window size, signal handling. This is the canonical surface for rich TUI applications (ratatui, crossterm).
- **WASM** (WASI preview 1) — sandboxed: inherited stdin/stdout, `poll_oneoff` for non-blocking I/O, clock subscriptions for sleep. No in-capsule terminal control API, no resize signals, and no crossterm/ratatui story. Full-screen terminal apps are still possible if the host provides raw mode plus terminal dimensions and the capsule renders ANSI directly.

This is a platform reality, not a design gap. App logic, command parsing, Carrier transport, and capability handling are shared across substrates. Only the UI rendering layer differs. A capsule like chat can ship both variants (`chat` for microVM TUI, `chat-stdio` for WASM ANSI TUI) from the same codebase.

## Recommended Language

Use these terms consistently:

- **Node Core** or **Runtime** for the trusted base
- **Carrier** for the decentralized substrate
- **Capsule Runtime** as the short practical name for AppCapsule Runtime
- **Digital Capsule** for the top-level object model

Avoid:

- using "Carrier" to mean the whole control plane
- using "runtime" to mean both node core and per-capsule runtime without qualification
- using "capsule" to mean only one substrate or only one process shape
