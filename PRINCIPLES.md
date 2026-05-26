# Principles

This file is the guiding-star contract for `elastos-runtime`.

It is not a roadmap.
It is the set of constraints that should decide ambiguous implementation choices.

## 1. Local First

The primary user-visible world is the local sovereign Home world.

That means:
- `localhost://...` is the local object model
- local state should not be explained primarily in terms of host paths, web servers, or cloud accounts
- public exposure is layered on top of local truth, not the other way around

## 2. Stable Identity Over Transport

Objects should be named by stable rooted or content identities, not by transport convenience.

That means:
- `localhost://...` and `elastos://...` are the real nouns
- HTTP URLs are delivery adapters, not canonical identity
- mutable heads must point to immutable objects
- a CID identifies content, not availability; replication and pinning promises must be explicit signed receipts

## 3. No Ambient Authority

Capsules, agents, and tools should not inherit ambient filesystem, network, or control authority.

That means:
- capabilities must be explicit
- authority must be narrow, auditable, and revocable
- missing authority should fail closed

## 4. Carrier Plane For Local And Off-Box

Capsule communication should default to the Carrier capability plane whether the
target is local or remote.

That means:
- capsules should know Carrier-style capability calls, not host routes, raw sockets, browser endpoints, or provider internals
- moving a target between same-runtime, same-device, LAN, or remote peer must not require capsule code changes
- local loopback, HTTP, WebSocket, `postMessage`, stdio, or in-process calls are host adapters below the capsule contract
- content publication should go through a content/availability provider contract, not raw IPFS/Kubo/gateway calls from app capsules
- ordinary public-web substitutes are a bug unless explicitly approved as edge adapters
- bootstrap exceptions must stay narrow, visible, and fail-closed
- trusted-source, signature, and content identity matter more than web location

## 5. Small Trusted Core

The runtime should stay small enough to reason about.

That means:
- trusted-core logic belongs in the runtime
- app logic belongs in capsules
- service logic belongs in providers or explicit system services
- host/web plumbing should not quietly become the product model

## 6. Clear User, Operator, and Developer Boundaries

The product must not blur normal user flows with operator and development flows.

That means:
- user commands should stay simple and human-facing
- operator commands should remain explicit
- developer/debug surfaces should not leak into the default mental model

## 7. Humans And Agents Share One Authority Model

Humans, bots, and AI should not get separate magical trust systems.

That means:
- `Users/...` and `UsersAI/...` are parallel concepts
- capabilities, audit, and resource boundaries should apply to both
- automation should be more explicit, not more ambient
- every visible user action should map to the same capability-scoped operation an agent would use
- pointer, keyboard, API, and Home-message paths must enforce the same authority boundary

## 8. WebSpaces Are Dynamic, Not Fake Storage

`WebSpaces` are not just folders with a new name.

That means:
- the resolver owns the moniker first
- `localhost://WebSpaces/<moniker>/...` is a dynamic interpreted handle
- file-like traversal is a result of resolution, not the starting assumption

## 9. HTTP Is Edge Transport, Not Product Truth

Browsers need HTTP/TLS, but ElastOS should own the meaning.

That means:
- gateway/edge owns public route meaning
- nginx/Caddy/etc. should be dumb front-door plumbing
- application/publication truth must live in rooted ElastOS state
- the same capsule may have multiple access paths, but only one product identity

## 10. One Canonical Path Per Operation

The repo should not hide multiple competing behaviors behind soft alternate paths.

That means:
- one runtime expectation per command
- one canonical install/update/publication path
- explicit failure when the intended path is not ready

## 11. Fail Closed, Then Explain

The system should prefer explicit failure over quiet degradation.

That means:
- no silent downgrade to weaker trust paths
- no pretending a feature is supported when it is only half-implemented
- error messages should explain what is missing and what the correct path is

## 12. Docs, Code, Tests, And Ops Must Agree

The architecture is only real when the repo surfaces teach the same contract.

That means:
- docs should describe actual behavior
- tests should enforce the intended boundary
- operator workflows should not depend on hidden exceptions
- drift should be treated as a bug

## 13. Objects, Capsules, And Spaces Must Stay Distinct

ElastOS should not collapse user things, software packages, and namespaces into one blurry noun.

That means:
- objects are the user's things: documents, songs, photos, games, sites, identities, revisions
- capsules are software roles that operate on objects: app, viewer, provider, shell, content
- spaces are where objects and services resolve: `localhost://...`, `elastos://...`, WebSpaces
- user-facing product surfaces should lead with human nouns such as `Home`, `Inbox`, `Library`, `Documents`, and `System`, not internal runtime jargon

## 14. Public Names Should Match Human Mental Models

The product should speak in words ordinary people can predict, not in internal runtime terms.

That means:
- `Apps` is the public term; `capsules` is the internal and developer term
- `System` is the operating surface; `Settings` and `Storage` are sections inside it
- raw paths, providers, and transport details should stay secondary to object identity
- one visible concept should have one primary name

## 15. Trust And Access Must Travel With Signed Content

Installable capsules and published objects should prove what they are and who may open them.

That means:
- trust should anchor in DID, CID, hash, and signature, not gateway location or host path
- IPLD-compatible object manifests should be preferred for published objects, signed heads, provenance, and availability receipts when content graphs need to be traversed or synchronized
- encrypted content should be normal, not a special exception
- decryption and license policy should be mediated by an explicit provider, not reimplemented inside every app
- the right abstraction is a capability-checked access/decryption plane that can use local storage, Carrier, or other substrates underneath without changing the capsule contract

## 16. UI Surfaces Must Not Be Authority

Opening a page and holding a capability are different things.

That means:
- Home may mint app launch capabilities only after the Home context itself has Home authority
- app, viewer, and provider APIs must require the capability for that surface, not trust route shape or iframe placement
- browser-frame messages into Home are orchestration requests, not capsule-to-capsule IPC; Home must bind them to the launched frame and its app-scoped capability before acting
- browser pairing grants a browser principal, not the native Home identity
- public summaries can show safe state but must not expose bearer tokens or mutation handles

## 17. Design Tokens Are Product Contracts

Visual consistency is part of system coherence, not decoration.

That means:
- Home owns the wallpaper and ElastOS brand layer
- first-party capsules share one light token set unless a functional surface needs a scoped exception
- colors should be named by role, not scattered as one-off literals
- UI copy, color, and action semantics should stay aligned across Home, System, Inbox, Library, Documents, Chat Room, and games

## Decision Rule

When two choices both work technically, prefer the one that:

1. strengthens rooted local and content identity
2. reduces ambient authority
3. removes hidden alternate-path and transport assumptions
4. keeps the trusted core smaller
5. makes the user model clearer
