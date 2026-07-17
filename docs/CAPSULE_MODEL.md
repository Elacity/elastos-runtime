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
- the per-capsule **Capsule Runtime** / AppCapsule Runtime
- the **Digital Capsule** as the portable signed package model

## Core Model

- **Digital Capsule**: the portable signed package.
  A capability-governed software role or sealed content package with explicit identity, interface, and lifecycle. It can carry a user object, but it is not the only way an object exists.
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
- Digital Capsule = portable app/service/content package
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
   - environment variables, bridge channels, syscalls, `carrier_invoke` calls, lifecycle conventions
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

A document can be a mutable local object while the user edits it, an immutable
published object when it has a CID, and a data capsule when it is sealed with
capsule metadata, provenance, and a declared viewer/handler for distribution.

## Capsule Taxonomy

Digital Capsule is the umbrella package term. In this repo, capsules fall into these categories:

- **Executable capsules**
  - **App capsule**: user-facing app such as Chat Room or Documents
  - **Provider capsule**: protocol implementation such as DID, localhost storage, tunnel, or AI
  - **Shell capsule**: orchestrator UI with policy authority via capability grants
  - **Agent capsule**: autonomous app capsule using the same capability model
- **Data capsules**
  - signed or content-addressed content packages with a viewer or handler

The important point is that these are all capsules. They differ by role, not by escaping the model.

## Capsule Runtime (AppCapsule Runtime)

AppCapsule Runtime should be understood as the common execution substrate for executable capsules.

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

## Isolated Capsule Execution Contract

Executable capsules run in resource-bounded, isolated execution environments.
A capsule artifact contains the application and the runtime dependencies it
needs above the stable Capsule Runtime contract. It does not inherit ambient
host authority or require a general-purpose guest operating system.

The normative formulation is:

> ElastOS does not launch ambient applications. It instantiates a signed and
> verified Digital Capsule for an explicit principal and session inside a
> resource-bounded, isolated execution environment. The capsule brings its
> application, language runtime or dependencies, and minimal capsule-local
> system surface; sees only the capability-secured ElastOS Bus; mounts mutable
> state through WebSpaces; and reaches peripherals or services through
> providers. Its artifact remains independently verifiable and
> re-instantiable on compatible ElastOS nodes, subject to the owner's current
> trust policy.

The contract separates these responsibilities:

| Responsibility | ElastOS contract |
|----------------|------------------|
| Workload isolation | A separately admitted capsule instance bound to a capsule identity, principal, session, capabilities, resources, and lifecycle |
| Package boundary | An immutable signed capsule artifact containing the app and the runtime dependencies needed above the stable Capsule Runtime contract |
| Guest ABI | The capsule kernel / ElastOS Bus, not ambient POSIX, host WASI, gateway routes, or a full host OS |
| Provider boundary | A typed provider contract, optionally backed by a narrow host adapter; raw device, protocol, credential, and topology details remain hidden |
| State mounts | A capability-scoped WebSpace or rooted object view; resolution may expose files, posts, people, identities, or services rather than pretending every space is a disk |
| User interface | Home and an ESP-compatible shell projection; the shell presents facts and requests verbs but is not the underlying authority |

The contract has these constraints:

- **One capsule instance is an isolation boundary, not necessarily one process
  forever.** A product may create multiple instances deliberately, but it must
  never silently share mutable execution state across principals.
- **The capsule-local runtime surface is small.** It is the guest library,
  scheduler or event loop, language support, and Bus bindings the app needs.
  Node scheduling, hardware isolation, principal management, and global policy
  remain Runtime responsibilities.
- **Self-contained does not mean state is baked into the executable.** The
  signed artifact is immutable; principal, app, and shared state are separately
  mounted, encrypted, synchronized, migrated, and revoked through object and
  WebSpace contracts.
- **Providers are interchangeable only at a shared typed interface.** Two drive
  providers may implement the same object operations. A social provider exposes
  typed people, post, and conversation objects rather than masquerading as a
  byte-for-byte drive.
- **Durable does not mean permanently executing or permanently trusted.** A
  historical artifact should remain identifiable and reproducible, while a
  current Runtime may still refuse it because its signature is revoked, its
  interfaces are unsupported, or policy marks it unsafe.
- **Vendor independence is a compatibility claim.** It requires a stable ABI,
  signed packages, portable state, and available artifacts. It does not mean the
  current repository is already an independent bootable appliance or can ignore
  the host kernel that runs the Runtime.
- **Browser and shell surfaces are projections.** An iframe, route, native
  window, terminal, or app-store listing can present a capsule, but none of
  those surfaces constitutes capsule authority or the capsule ABI.

### Readiness Proof

The isolated-execution claim is earned only when one reusable acceptance path
can:

1. fetch an immutable capsule without depending on its original vendor or app
   store, then verify its full bundle identity, publisher, signatures,
   provenance, interfaces, and declared resources;
2. admit it for an explicit principal and session without conflating principal,
   device, capsule, proof binding, launch grant, or session identity;
3. enforce declared memory, compute, time, instance, storage, and egress bounds,
   including cancellation, stop, cleanup, and truthful status;
4. prove that ordinary product code can import only the versioned ElastOS Bus
   contract and receives no ambient environment, host files, preopens, sockets,
   gateway routes, provider credentials, or raw protocol authority;
5. mount principal and shared state through capability-scoped object and
   WebSpace contracts, with isolation, encryption, quota, sync, conflict,
   migration, and recovery behavior explicit;
6. exchange one provider for another implementing the same signed interface
   without changing capsule code or revealing backend topology;
7. reject tampered, revoked, incompatible, over-budget, or incompletely
   authorized capsules before an effect occurs, with an auditable reason; and
8. re-instantiate the same historical artifact and compatible state on another
   compatible ElastOS node while preserving identity and migration receipts.

Until that path passes, isolated capsule execution is an architecture contract
and a directional description of partial implementation, not a
release-readiness claim. Current proof and gaps belong in
[../state.md](../state.md) and [../TASKS.md](../TASKS.md), not in this contract.

## Capsule Kernel / ElastOS Bus

Each executable capsule should boot with a tiny capsule-local system surface: the
capsule kernel. This is not the node core and not a general-purpose OS kernel.
It is the in-capsule ABI/SDK that lets capsule code ask ElastOS for effects
without learning host topology.

The executable product capsule ABI is `elastos.component/v1`. Product capsules
that execute as WASM are Components that import only the interfaces declared by
the `elastos:bus@v1` contract. WASI Preview 1 is not a supported product ABI for
this branch.

The capsule kernel should expose only the stable ElastOS contract:

- capability state: inspect granted capabilities and request missing authority
- provider/resource invoke by resource URI and operation
- runtime info
- identity context
- audit context: request ID, principal, session, capsule identity, and reason strings
- cancellation

It should not expose product-facing access to gateway routes, host files, raw
node RPC, browser-only APIs, IPFS/Kubo APIs, wallet RPC, node RPC, TAP devices,
or provider implementation details.

When a launched capsule needs a user-root principal, the launcher must provide a
runtime-verified launch grant. Home-backed launches use a signed, app-scoped,
non-delegatable Home launch token as that grant; raw principal strings are not a
launch authority.

The Rust guest SDK (`elastos-guest`) intentionally exposes only this
capsule-kernel lane: capability requests, `carrier_invoke` calls, runtime info,
and ping. Shell/runtime-control operations such as list, launch, stop, grant,
revoke, direct storage, direct provider routing, and direct capsule messaging
are not capsule-kernel API.

The older `elastos-runtime::handler` protocol is an internal shell/control and
legacy stdio bridge surface. It may keep privileged orchestration operations,
but it is not the ordinary app capsule SDK and must not be documented as one.

Manifest validation enforces this for ordinary app, viewer, and content
capsules: they may not request guest networking, host execution, microVM HTTP
ports, external host dependencies, provider-source overrides, protocol provider
namespaces, system-only backend namespaces such as raw gateway/IPFS/Kubo/Elacity
provider surfaces, or runtime SystemServices storage. Provider capsules are the
explicit exception and must declare a narrow `provides` namespace plus
provider-authority metadata: reason, capability schema, operations, and expected
audit events.

The same capsule kernel call may route to:

- a local object or provider in the same runtime
- another capsule on the same node
- a remote runtime over Carrier
- a provider capsule that owns a protocol such as IPFS, BTC, ELA, DID, or WebSpace

The capsule must not branch on those cases. The runtime and provider plane own
routing, authorization, transport, and audit. This is the practical meaning of
"capsules know only Carrier."

DID signing follows the same rule. Provider capsules may hold DID material, but
ordinary capsules must request typed signing intents such as `sign_chat_message`
instead of arbitrary `sign(data)` access.

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
   - `elastos://peer/<verified-peer>/shared` is named data, not a filesystem path
   - provider capsules define how that namespace is interpreted

## Trust Domains

Capsules operate in one of two trust domains:

### User/Application Domain

App capsules, agent capsules, and user-facing provider capsules run in the **user trust domain**:

- Shell-mediated capability approval (pending request → shell grant/deny)
- Full capability token flow with Ed25519-signed tokens
- Subject to shell policy (auto, cli, or agent/rules modes)
- Bridge provides `BridgeContext` with `PendingRequestStore` and `CapabilityManager`
- User-root aliases such as `localhost://Users/self` require an explicit
  Runtime-verified principal context. Home-backed WASM launches and
  shell/supervisor microVM launches carry that context through signed
  app-scoped launch grants; attached/native CLI launches remain principal-less
  until they get the same protected bridge.

This is the normal path for `elastos serve` + `elastos run`/`elastos chat`.

### Infrastructure/Service Domain

Gateway-launched capsules (ipfs-provider, tunnel-provider) run in the **infrastructure trust domain**:

- Trusted service-plane components launched by the runtime operator
- Not subject to user shell approval — they ARE the service infrastructure
- No `CapabilityManager` or `PendingRequestStore` attached
- If an infrastructure capsule ever requests a capability, the bridge returns a clear `infrastructure_capsule` denial
- Provider-role launches do not receive user principal scope from launch grants;
  a provider that needs user data must request it through a separate capability
  path.
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
- the node brokers allowed effects through `carrier_invoke` calls and capability grants

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

## SmartWeb Alignment

Two SmartWeb capsule principles are central here:

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
- `localhost://WebSpaces/<mount>/...` is the local mounted view; raw provider
  targets such as `cloud://drive/...` or backing `elastos://content/...`
  handles remain provider-resolved authority, not ordinary app storage

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
- **WASM Component** — loaded through `elastos.component/v1` and constrained by the `elastos:bus@v1` WIT world. Storage, networking, wallet, and provider effects must be explicit Runtime/provider capabilities, not inherited substrate authority. ElastOS Bus v1 does not expose streams. Home CLI's full-screen terminal uses a separate Runtime-owned, launch-token-gated terminal contract; a future Bus version may add streams only after they share the same authorization, audit, capacity, and lifecycle path.

This is a platform reality, not a design gap. App logic, command parsing, Carrier transport, and capability handling can be shared across substrates. Product capsule IDs should still describe user-facing intent, not the implementation substrate.

## Recommended Language

Use these terms consistently:

- **Node Core** or **Runtime** for the trusted base
- **Carrier** for the decentralized substrate
- **Capsule Runtime** as the short practical name for AppCapsule Runtime
- **Digital Capsule** for the portable signed package model

Avoid:

- using "Carrier" to mean the whole control plane
- using "runtime" to mean both node core and per-capsule runtime without qualification
- using "capsule" to mean only one substrate or only one process shape
