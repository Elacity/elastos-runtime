# Principles

This file defines the stable implementation constraints for `elastos-runtime`.
It is not a roadmap. Use these constraints to resolve ambiguous implementation
choices.

## 1. Local first

The user-facing environment starts with the user's local Home.

- `localhost://...` is the local object model
- local state should not be explained primarily in terms of host paths, web servers, or cloud accounts
- public exposure derives from local state

## 2. Stable identity over transport

Objects should be named by stable rooted or content identities, not by transport convenience.

- `localhost://...` and `elastos://...` are the canonical identities
- HTTP URLs are delivery adapters, not canonical identity
- mutable heads must point to immutable objects
- a CID identifies content, not availability; replication and pinning promises must be explicit signed receipts

## 3. No ambient authority

Capsules, agents, and tools should not inherit ambient filesystem, network, or control authority.

- capabilities must be explicit
- authority must be narrow, auditable, and revocable
- missing authority should fail closed

## 4. Carrier is not the capsule contract

Executable capsules request effects through typed, capability-secured Runtime
resources whether the target is local or remote. Components use ElastOS Bus.
Web projections use narrow Runtime adapters under the same authority
contract. Runtime owns authority, routing, lifecycle, and audit; providers own
operation and protocol semantics. Carrier is the endpoint-authenticated
transport for off-box peer or content communication when Runtime routing
selects it, not the capsule API or a required local envelope.

- executable capsule code should know typed Runtime resources, not host routes, raw sockets, browser endpoints, or provider internals
- moving a target between same-runtime, same-device, LAN, or remote peer must not require capsule code changes
- local loopback, HTTP, WebSocket, `postMessage`, stdio, or in-process calls are host adapters below the capsule contract
- Carrier remains the endpoint-authenticated off-box transport, discovery, replication, and content-delivery plane behind those resources
- Carrier endpoint authentication proves the transport peer, not the author of an application message; message authorship requires its own verified signature or authority binding
- private-network membership and service discovery are routing inputs, not capabilities; the destination Runtime independently authorizes the requested service and operation
- legacy IP, TUN, Exit, and LAN Gateway adapters must stay behind explicit Runtime policy and must not become ambient capsule networking
- content publication should go through a content/availability provider contract, not raw IPFS/Kubo/gateway calls from app capsules
- ordinary public-web substitutes are a bug unless explicitly approved as edge adapters
- bootstrap exceptions must stay narrow, visible, and fail-closed
- trusted-source, signature, and content identity matter more than web location

## 5. Small trusted core

The runtime should stay small enough to reason about.

- trusted-core logic belongs in the runtime
- app logic belongs in capsules
- service logic belongs in providers or explicit system services
- host/web plumbing should not quietly become the product model

## 6. Clear user, operator, and developer boundaries

The product must not blur normal user flows with operator and development flows.

- user commands should stay simple and human-facing
- operator commands should remain explicit
- developer/debug surfaces should not leak into the default mental model

## 7. Humans and agents share one authority model

Humans, bots, and AI use the same trust system.

- `Users/...` and `UsersAI/...` are parallel concepts
- capabilities, audit, and resource boundaries should apply to both
- automation should be more explicit, not more ambient
- every visible user action should map to the same capability-scoped operation an agent would use
- pointer, keyboard, API, and Home-message paths must enforce the same authority boundary

## 8. WebSpaces resolve names dynamically

`WebSpaces` are resolver-owned handles, not ordinary folders.

- the resolver owns the moniker first
- `localhost://WebSpaces/<moniker>/...` is a dynamic interpreted handle
- file-like traversal is a result of resolution, not the starting assumption
- names, paths, mounts, and resolver handles do not grant resource authority
- a capsule-visible resource view derives from active Runtime grants and
  provider bindings; every operation still requires its Runtime-verified
  capability

## 9. HTTP is edge transport

Browsers need HTTP/TLS, but ElastOS should own the meaning.

- gateway/edge owns public route meaning
- nginx, Caddy, and similar tools should only proxy the public routes
- application/publication truth must live in rooted ElastOS state
- the same capsule may have multiple access paths, but only one product identity

## 10. One canonical path per operation

The repo should not hide competing behaviors behind undocumented fallbacks.

- one runtime expectation per command
- one canonical install/update/publication path
- explicit model-provider selection and fallback policy for inference
- explicit failure when the intended path is not ready

## 11. Fail closed, then explain

The system should prefer explicit failure over quiet degradation.

- no silent downgrade to weaker trust paths
- no pretending a feature is supported when it is only half-implemented
- error messages should explain what is missing and what the correct path is

## 12. Docs, code, tests, and operations must agree

Documentation, code, tests, and operator workflows must describe the same
contract.

- docs should describe actual behavior
- tests should enforce the intended boundary
- operator workflows should not depend on hidden exceptions
- drift should be treated as a bug

## 13. Objects, capsules, and spaces must stay distinct

ElastOS must distinguish user objects, software packages, and namespaces.

- objects are the user's things: documents, songs, photos, games, sites, identities, revisions
- Digital Capsules are complete, portable signed packages; admission is a
  separate decision made under each Runtime's trust policy
- `app`, `viewer`, `provider`, and `shell` are executable roles, while
  `content` carries portable data under a data contract
- source packages and development projections are build inputs, not complete
  signed Digital Capsules or evidence of portable installation
- spaces are where objects and services resolve: `localhost://...`, `elastos://...`, WebSpaces
- user-facing product surfaces should lead with human nouns such as `Home`, `Inbox`, `Library`, `Documents`, and `System`, not internal runtime jargon

## 14. Public names should match human mental models

The product should speak in words ordinary people can predict, not in internal runtime terms.

- `Apps` is the public term; `capsules` is the internal and developer term
- `System` is the operating surface; its sections should represent real user controls, not placeholder implementation categories
- raw paths, providers, and transport details should stay secondary to object identity
- one visible concept should have one primary name

## 15. Trust and access must travel with signed content

Installable capsules and published objects need verifiable identity and explicit
access policy.

- use CIDs and hashes for content integrity, and verified DIDs and signatures
  for publisher identity
- use IPLD-compatible manifests for published objects, signed heads,
  provenance, and availability receipts when content graphs need traversal or
  synchronization
- encrypted content must use the normal content path
- decryption and license policy should be mediated by an explicit provider, not reimplemented inside every app
- Runtime may route a capability-checked access/decryption request through a
  private local adapter, Carrier, or a provider-internal compatibility
  substrate without changing the capsule contract; the calling capsule does
  not select the peer, backend, or transport
- when a first-party production dKMS service crosses an ElastOS machine
  boundary, Carrier is its canonical off-box transport; dKMS authorization and
  end-to-end cryptography remain above Carrier, and compatibility transports
  stay provider-internal

## 16. UI surfaces must not be authority

Opening a page and holding a capability are different things.

- Home may request an app launch token only after its context has Home authority; Runtime validates the request and mints the token
- app, viewer, and provider APIs must require the capability for that surface, not trust route shape or iframe placement
- browser-frame messages into Home are orchestration requests, not capsule-to-capsule IPC; Home must bind them to the launched frame and its capsule-scoped capability before acting
- browser pairing grants a browser principal, not the native Home identity
- public summaries can show safe state but must not expose bearer tokens or mutation handles

## 17. Design tokens are product contracts

Visual consistency helps users understand which controls and states mean the
same thing.

- Home owns the wallpaper and ElastOS brand layer
- first-party capsules share token roles and interaction semantics; a functional
  surface may use a scoped palette
- colors should be named by role, not scattered as one-off literals
- UI copy, color, and action semantics should stay aligned across Home, System, Inbox, Library, Documents, Chat Room, and games

## 18. Executable capsules are isolated execution environments

ElastOS should instantiate explicit, resource-bounded capsule instances, not
launch applications with ambient host authority.

- every executable capsule instance is bound to a capsule identity, session, capabilities, resources, and lifecycle; user-scoped authority also requires a verified principal
- a capability token does not prove principal or session authority; Runtime
  verifies that context separately when an effect uses user-scoped authority
- an executable capsule brings its application, language runtime or dependencies, and a minimal capsule-local system surface; the host OS is not its product ABI
- capsule effects cross capability-secured Runtime resources and provider interfaces, never raw host topology
- mutable state is separate from the immutable capsule artifact and is mounted through rooted objects and WebSpaces
- portability means independently verifiable and re-instantiable on compatible ElastOS nodes under current trust policy, not an immortal process or a bypass of revocation
- shells, browser frames, routes, and app stores may project or distribute capsules, but they never become capsule identity or authority

## 19. Consequences govern effects, not transports

ElastOS uses one typed Runtime effect path for digital, economic, rights, and
physical operations. The operation's consequences determine its authority,
approval, retry, settlement, and safety requirements. HTTP, Carrier, a webhook,
a field bus, or an in-process call does not.

- Runtime policy sets and enforces the minimum classification from the admitted provider operation contract; capsule-declared risk may tighten that policy but cannot weaken it
- observations used for authority or safety decisions must bind their source, subject, schema, time, freshness, integrity, and replay boundary
- retry-sensitive effects require an idempotent contract or a durable effect identifier that can be reconciled before retry
- transport acceptance, provider acceptance, execution, and observed outcome are separate claims; uncertainty after dispatch must remain explicit
- a destination Runtime independently authorizes a remote operation, and a physical controller retains the final local safety decision
- a DID, ownership record, right, or payment proof may inform policy but does not become control authority by itself
- Runtime authorizes, routes, reconciles, and audits; hard real-time control, interlocks, emergency stops, and safe-state behavior remain local to the device provider or controller

The detailed contract is [Consequence-aware effects](docs/CONSEQUENCE_AWARE_EFFECTS.md).

## Decision rule

When two choices both work technically, prefer the one that:

1. strengthens rooted local and content identity
2. reduces ambient authority
3. removes hidden alternate-path and transport assumptions
4. keeps the trusted core smaller
5. makes the user model clearer
