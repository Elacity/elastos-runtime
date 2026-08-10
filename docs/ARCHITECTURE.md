# ElastOS architecture

This document describes the intended system architecture. For current behavior,
proof level, and command expectations, see [state.md](../state.md) and
[COMMAND_MATRIX.md](COMMAND_MATRIX.md). Verified security findings belong in
[SECURITY.md](../SECURITY.md), and open work belongs in [TASKS.md](../TASKS.md).

## Architectural direction

ElastOS keeps the trusted Runtime small. Ordinary executable capsules run
without ambient authority and request scoped effects through Runtime-owned
capability checks. Providers implement service and protocol behavior outside
the trusted core. Content hashes establish integrity, not publisher identity,
authority, or availability.

[PRINCIPLES.md](../PRINCIPLES.md) owns these rules. The
[glossary](GLOSSARY.md) owns product terminology, and
[CAPSULE_MODEL.md](CAPSULE_MODEL.md) supplements this document with capsule
layers and lifecycle.

## ElastOS Four Quadrants

The four quadrants divide responsibility within the ElastOS World Computer
model. Together they describe one product with one trusted-core boundary. The
division keeps protocol behavior out of Runtime and prevents apps from
regaining ambient internet authority.

| Quadrant | Responsibility | In this repo | Must not become |
|----------|----------------|--------------|-----------------|
| Home (PC2) | Human front door, object browser, spaces, people, app install/launch UX | Home, System, Inbox, Library, Documents, browser host adapters | trusted-core policy logic or protocol implementation |
| Runtime | Isolation, verification, principals, sessions, capabilities, object routing, audit | `elastos` Runtime core, capsule launch, provider routing, Home authority checks | app business logic, social-network bridge, wallet app, or storage backend |
| Carrier | Endpoint-authenticated transport for objects, messages, streams, discovery, sync, replication, and content delivery | Carrier abstraction and provider-facing transport contracts | chat-only transport, raw gossip exposed to apps, or replacement for capabilities |
| Blockchain | DID/EID, wallet signing, provenance anchors, publisher identity, receipts/licensing hooks | Runtime-facing provider boundary for identity/provenance operations | app database, mandatory UX blocker, DeFi-first layer, or runtime business logic |

Executable effects enter Runtime through a substrate-specific surface.
Components use ElastOS Bus. Web projections and MicroVMs use narrow,
capsule-scoped Runtime adapters. Every surface enters Runtime's authority and
routing boundary. Runtime handles core operations directly and sends
provider-backed effects through the provider registry.

Discovery and trust records do not grant authority. Runtime owns authority,
routing, lifecycle, and audit; providers own operation and protocol semantics.
Carrier is an endpoint-authenticated off-box transport selected below Runtime
routing, not a capsule API. Endpoint authentication does not prove message
authorship.
[Principle 4](../PRINCIPLES.md#4-carrier-is-not-the-capsule-contract) owns the
full boundary. See [CARRIER.md](CARRIER.md) and
[BROWSER_CAPSULE.md](BROWSER_CAPSULE.md) for their concrete contracts.

This is the implemented contract shape, not a claim that every product App has
migrated substrates. In the 0.6 review tree, the Component runner is exercised
by a conformance fixture and authoring template; all shipped first-party UI Apps
remain `elastos.runtime-projection/v1` web projections.

A verified signed collaboration-network profile supplies the Runtime's bounded
bootstrap peers and authenticates the content-addressed default-conversation
grant. The Runtime collaboration service, durable core, and transport driver own
that authority and give Carrier only opaque signed envelopes. Configured Chat
text receives typed projections from the Runtime-owned product port and never
receives tickets, endpoints, raw sockets, or Carrier topics. The route-owned Room
gossip exception is gone; People/discovery migration remains separate open work.

Sequencing and incomplete work belong in [ROADMAP.md](../ROADMAP.md) and
[TASKS.md](../TASKS.md). This document defines the following authority
and recovery invariants:

- A passkey proves local principal authority. It is not a DID, wallet key,
  decryption key, or source of ambient administrator authority.
- Each principal owns a distinct `localhost://Users/<principal-root>` area.
  Closing enrollment does not revoke existing principals.
- Removing a credential revokes access, not storage. Recovery or reassignment
  must prove the exact principal, root, and protection key before changing
  bindings or data.
- Recovery must validate every declared protected object before mutation,
  revoke replaced sessions, and leave an audit trail. It must not fall back to
  a device-global key.
- Protection configured, root encrypted, and root recoverable are separate
  claims. Current coverage and exclusions belong in [state.md](../state.md).

### Identity and object claims

The [glossary](GLOSSARY.md) defines principals, passkeys, DIDs, CIDs, IPLD, and
human-readable names. The architecture keeps their authority claims separate.

Do not add a `did:localhost` shortcut unless ElastOS defines and implements a
DID method for it. Local accounts already have runtime principals and
`localhost://Users/<principal-root>` roots. If a user wants to claim `alice`
globally, that claim must go through an EID/DID-chain namespace or equivalent
registry with consensus-backed uniqueness, transfer, expiry, recovery, and
signed ownership. Local display handles can collide; global names cannot.

An authority decision must verify every principal ID, DID, signature, CID, or
signed head it relies on. An unverified handle string cannot carry authority. A
CID does not need its own DID by default. A mutable object or collection may
later have a signed object identity or object DID that points to successive CID
revisions.

WebAuthn relying-party scope is part of the authority contract. Hosted Home
uses the hosted HTTPS origin; localhost development uses loopback HTTP origins;
installed PWAs inherit their install origin. These worlds do not share passkeys
unless a future native/mobile adapter is modeled as an explicit proof adapter.
Malformed or insecure browser origins must fail closed instead of falling back
to the runtime host authority.

Published objects should use IPLD-compatible content graphs when the graph aids
traversal, provenance, signed heads, or availability receipts. IPLD is the
object graph model. Carrier provides coordination and transport. The content
provider owns publish, fetch, status, and repair policy. See
[CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md).

## Architecture overview

```text
Person or agent
    |
Home front door and active shell projection
    |
Runtime launch and orchestration authority
    |
isolated executable capsule instance
    |
    +-- Component ----------------> ElastOS Bus ------------+
    |                                                      |
    +-- Web / MicroVM / other --> narrow Runtime adapter ---+
                                                           |
                                                           v
                                          Runtime effect authority
                                          and routing
                                                           |
                                    +----------------------+------------------+
                                    |                                         |
                                    v                                         v
                         Runtime-owned operation                    provider-backed effect
                                                                              |
                                                                              v
                                                                   provider operation
                                                                              |
                                                                              v
                                           local host adapter or Runtime-selected Carrier route
```

Ordinary executable capsules run in isolated environments. Their manifests
declare an execution or data contract but grant no authority. Exact fields and
accepted combinations belong in [CAPSULE_AUTHORING.md](CAPSULE_AUTHORING.md).
The Capsule Runtime contract binds an artifact, session, capabilities,
resources, state, and lifecycle. User-scoped authority also requires a verified
principal. The contract should remain stable across compatible substrates.

### Host adapters

The target architecture keeps the capsule contract stable while host adapters
present capsule output differently. An adapter may use a browser, native window,
embedded webview, or device-owned display. These are presentation choices, not
separate capsule contracts or support claims.

Capsules depend on the Capsule Runtime contract, not a presentation host. A web
projection declares its browser entrypoint and projection metadata; the host
adapter decides how to present it. See [INSTALL.md](INSTALL.md) and
[state.md](../state.md) for current platform evidence.

Keep these concerns separate:

- Launch authority determines who may start a capsule.
- Session authority determines what the caller may do after connecting.
- The surface adapter determines how the capsule UI is rendered.

A web surface is only a rendering adapter. It does not define identity or
authority. The [Home shell contract](HOME_SHELL_HOST_CONTRACT.md) owns sign-in,
shell selection, child-message routing, launch grants, and browser-session
behavior. [CARRIER.md](CARRIER.md) owns peer admission and member/guest
boundaries.

## The three layers

### Layer 1: Runtime (`elastos` binary)

The Runtime core is the host-side enforcement authority. It owns isolation,
signature verification, capability enforcement, trusted object routing, and
the lifecycle needed to maintain those guarantees. Work that does not need
that authority belongs in a capsule, provider, or explicit operator service.
The Capsule Runtime is the per-capsule execution surface, not the host Runtime
core.

### Layer 2: Home host and shell projections

Home owns the user-facing sign-in boundary and presents Runtime facts through
shell projections. A shell role is descriptive; it grants no authority.
Runtime admits installed identities and checks each requested effect. See
[HOME_SHELL_HOST_CONTRACT.md](HOME_SHELL_HOST_CONTRACT.md) for the current
identity, lifecycle, and intent contract.

Interaction equality is part of the same rule: a visible human action and an
agent request must reach the same capability-scoped Runtime operation.

Home shell browser state may persist safe window and route descriptors for
restoration, but not Browser VM Chromium profile disks, launch tokens,
authority objects, or provider state. Restored windows must reacquire their
Runtime lifecycle normally.

### Layer 3: Capsules

Ordinary capsules contain application, viewer, shell, or content behavior.
Provider capsules are explicit service-plane exceptions with narrow declared
authority. Package roles, types, execution ABIs, and checked authoring examples
belong in [CAPSULE_AUTHORING.md](CAPSULE_AUTHORING.md). All capsule effects
still cross Runtime authority checks.

### Boundary decisions

These decisions keep the layers from collapsing into one another:

| Boundary | Consequence |
| --- | --- |
| Trusted core | Runtime contains only enforcement that must be trusted. UI and protocol policy remain outside it. |
| Capsule ABI | The checked Component and Bus contracts are the executable Component path. WASI Preview 1 is rejected at product admission. Shipped first-party UI Apps remain web projections until migrated explicitly. |
| Host adapters | HTTP, browser messaging, loopback, stdio, and in-process calls may implement an adapter, but do not become capsule APIs. |
| Provider namespace | A provider exposes a typed Runtime-facing contract. Its protocol, credentials, topology, and local bridges stay behind that contract. |
| Release trust | Runtime verifies expected hashes, signatures, and publisher policy regardless of the transport that delivered an artifact. |
| Carrier | Carrier is the endpoint-authenticated transport selected by Runtime for peer, message, stream, and content traffic. It does not prove message authorship, grant capabilities, or replace local dispatch. |

A boundary is still present when every component runs on one machine. Local
placement may remove a network hop, but it must not remove caller validation,
capability checks, provider policy, or audit. Conversely, moving a provider or
engine off-box must not give ordinary capsules a transport-specific API.
Provider discovery can offer a candidate endpoint; only an explicit,
principal-scoped grant can make it usable.

## Capsule network model

The network boundary requires:

- Ordinary executable capsules have no ambient network.
- Components use ElastOS Bus; web projections use narrow Runtime
  adapters.
- Carrier and providers mediate allowed external communication.

Launch policy follows:

- Normal app capsules should launch rootless, without a TAP device or guest IP
  requirement.
- Internet access should require an explicit Runtime capability rather than a
  default NIC inside the capsule.
- Guest networking remains an explicit compatibility mode for provider capsules
  that require raw TCP or guest-facing network services.

Executable capsules express intent, and Runtime decides how to fulfill it.

## WebSpace addressing

[NAMESPACES.md](NAMESPACES.md) defines accepted schemes, current roots, and URI
examples. Runtime checks capabilities and dispatches namespace operations;
providers keep backend identifiers, credentials, synchronization, and transport
below stable handles. HTTP and tunnels are edge transports, not trust
boundaries. [CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md) owns publication,
fetch, repair, backend selection, IPLD graphs, and availability receipts.

## Trusted and encrypted content

Integrity, publisher trust, availability, rights, key release, and decryption
are separate claims. Runtime checks authority and routes each protected-content
operation to its owning provider. Ordinary capsules receive neither raw keys
nor provider credentials. [PROTECTED_CONTENT.md](PROTECTED_CONTENT.md) owns the
access sequence and failure rules; [CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md)
owns publication and retrieval.

## Capability system

Runtime validates a signed token against the capsule caller, action, resource,
and constraints before provider routing. The token does not prove principal or
session authority. Runtime verifies that context separately for user-scoped
effects.

[PRINCIPLES.md](../PRINCIPLES.md), [ESP v0](ESP_V0.md), and the
[Capsule interface contract](CAPSULE_INTERFACE_CONTRACT.md) define the
authority boundary. Current token fields and ordered validation are enforced by
Runtime code and tests; this architecture document does not duplicate them.

## Security model

### Trust via content addressing

A CID lets Runtime detect whether fetched bytes match a trusted content
reference. It does not establish who published the reference, whether the
content is current, or whether it will remain available. Admission therefore
also verifies the expected publisher, signatures, manifest, and policy. Mutable
heads need their own rollback and freshness rules.

### Trust levels

A trust label may inform admission policy and audit, but it grants no
authority. Runtime must still verify the artifact signature against publisher
policy and validate every requested effect.

### Defense in depth

| Layer | Protection |
|-------|------------|
| Content hash | Integrity of the referenced bytes |
| Signature | Control of a signing key; publisher policy identifies the trusted signer |
| Sandbox | Isolation between executions |
| Capabilities | Authority for a checked resource operation |
| Encryption | Confidentiality only for content covered by the relevant key contract |

## Boot sequence

1. Runtime loads its local identity and installed-component metadata. Each
   authority subsystem loads only the state its contract marks durable.
2. The trusted Runtime core initializes isolation, verification, capability
   enforcement, object routing, and the provider registry.
3. Runtime serves the neutral Home front door. Principal and child-app
   authority remain unavailable until their required proofs and scoped grants
   validate.
4. Runtime accepts requests, validates capabilities, manages capsule lifecycle,
   and routes authorized effects to providers.

## Runtime interface

The Component capsule contract is the checked `elastos:bus@v1` WIT surface, not
a conceptual API in this document. Web projections, MicroVMs, and other
substrates use documented, capsule-scoped Runtime adapters. Every ingress
surface enters Runtime's authority and routing boundary. Runtime-owned
operations terminate there; provider-backed effects use the provider registry.

Orchestration surfaces can request lifecycle actions and present grant review.
Ordinary capsules can invoke only the resources covered by their scoped
authority. Runtime alone mints grants, records trusted audit facts, and owns
lifecycle cleanup. See [ESP_V0.md](ESP_V0.md),
[CAPSULE_INTERFACE_CONTRACT.md](CAPSULE_INTERFACE_CONTRACT.md),
[HOME_SHELL_HOST_CONTRACT.md](HOME_SHELL_HOST_CONTRACT.md), and the WIT
definitions under `elastos/wit/` for exact operations.

## Provider capsule interface

Provider capsules handle provider-scoped requests behind the Runtime provider
registry. Provider-specific transports and behavior remain behind the Runtime
boundary.

Provider namespaces and operations belong in their contract documents, not in
a universal CRUD interface or hand-maintained architecture table. New provider
families must define a typed contract before becoming visible in Home. The
[documentation index](README.md) lists the current provider contracts.

## Orchestration and Runtime communication

Home and other orchestration surfaces request actions through scoped Runtime
interfaces. Runtime validates the caller and mints target-scoped grants;
transport, route, and UI placement do not confer authority. The exact Home
messages, browser-origin checks, and launch-token rules belong in
[HOME_SHELL_HOST_CONTRACT.md](HOME_SHELL_HOST_CONTRACT.md).
