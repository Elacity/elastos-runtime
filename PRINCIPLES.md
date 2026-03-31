# Principles

This file is the guiding-star contract for `elastos-runtime`.

It is not a roadmap.
It is the set of constraints that should decide ambiguous implementation choices.

## 1. Local First

The primary user-visible world is the local sovereign PC2 world.

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

## 3. No Ambient Authority

Capsules, agents, and tools should not inherit ambient filesystem, network, or control authority.

That means:
- capabilities must be explicit
- authority must be narrow, auditable, and revocable
- missing authority should fail closed

## 4. Carrier First Off-Box

Off-box Elastos communication should default to Carrier and trusted-source paths, not public-web convenience.

That means:
- ordinary public-web fallback is a bug unless explicitly approved
- bootstrap exceptions must stay narrow and visible
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

## 10. One Canonical Path Per Operation

The repo should not hide multiple competing behaviors behind soft fallback.

That means:
- one runtime expectation per command
- one canonical install/update/publication path
- explicit failure when the intended path is not ready

## 11. Fail Closed, Then Explain

The system should prefer explicit failure over quiet degradation.

That means:
- no silent fallback to weaker trust paths
- no pretending a feature is supported when it is only half-implemented
- error messages should explain what is missing and what the correct path is

## 12. Docs, Code, Tests, And Ops Must Agree

The architecture is only real when the repo surfaces teach the same contract.

That means:
- docs should describe actual behavior
- tests should enforce the intended boundary
- operator workflows should not depend on hidden exceptions
- drift should be treated as a bug

## Decision Rule

When two choices both work technically, prefer the one that:

1. strengthens rooted local and content identity
2. reduces ambient authority
3. removes fallback and hidden transport assumptions
4. keeps the trusted core smaller
5. makes the user model clearer
