# Digital Capsule model

[GLOSSARY.md](GLOSSARY.md) defines the canonical terms, and
[CAPSULE_AUTHORING.md](CAPSULE_AUTHORING.md) defines
manifest fields and supported combinations.

For system context, see the [repository README](../README.md) and
[ARCHITECTURE.md](ARCHITECTURE.md). [state.md](../state.md) records verified
implementation status, and [TASKS.md](../TASKS.md) records open work.

## Core model

A Digital Capsule is a portable, signed software or sealed-content package
with an identity, interface, and lifecycle. Runtime admits and runs executable
capsules. Carrier handles endpoint-authenticated peer and content transport
when Runtime selects an off-box route.

## Capsule layers

The canonical model keeps five layers separate. A capsule need not use all
five.

| Layer | Meaning |
| --- | --- |
| Artifact | Immutable manifest-and-payload closure, normally named by content ID. A verified signature authenticates its publisher; provenance records describe claimed lineage. |
| Runtime contract | Declared execution or data contract. Component artifacts name a versioned ABI and Bus surface. Host adapters remain below it. |
| Instance | For executable artifacts, one admitted execution bound to a session, capabilities, resources, and substrate. User-scoped authority also binds a verified principal. |
| State | Mutable principal, app, or shared data stored outside the immutable artifact. |
| Head | Optional mutable pointer to a preferred immutable version. It is a publication model, not a required manifest field. |

Verification, migration, revocation, and reproduction depend on keeping these
layers separate.

A document remains a mutable local object while the user edits it. Once
published with a CID, that revision is immutable. It becomes a distributable
data capsule when sealed with capsule metadata and provenance. A viewer binding
is optional and belongs in the content contract only when required.

Games, GGUF models, and similar downloadable data use the same rule. Their
canonical package identity is the CID of the complete manifest-and-payload
closure. A signed catalog entry points to that CID, an availability receipt
states who retains it, and an installed inventory records local admission.
Those records must not become competing package identities.

Content distribution is distinct from service discovery. A GGUF content capsule
does not publish `elastos.service.offer/v1`; a running model provider may publish
an inference offer after Runtime admits the model. The full Get, bootstrap, and
external-gateway contract is in
[Content capsule distribution](CONTENT_CAPSULE_DISTRIBUTION.md).

## Isolation boundary

Runtime admits an executable artifact for a session and binds the instance to
declared resources and capabilities. User-scoped authority also requires a
verified principal. The instance does not inherit host files,
sockets, credentials, routes, or provider internals. Mutable state enters
through capability-scoped object and WebSpace contracts.

Roles, package types, ABI fields, provider declarations, and rejected
combinations belong to [Capsule authoring](CAPSULE_AUTHORING.md).

## Capsule kernel contract

A Component capsule receives the `elastos:bus@v1` capsule-kernel surface. It is
the in-capsule ABI used to request effects without exposing host topology. The
host Runtime core and any general-purpose OS remain separate layers.

The imported surface is limited to:

- capability requests
- provider invocation by resource URI and operation
- runtime info
- identity context
- an optional audit request ID in provider responses

The capsule exports `lifecycle.run`.

Component capsules do not receive gateway routes, host files, browser-only APIs,
IPFS/Kubo APIs, wallet or node RPC, TAP devices, or provider implementation
details through this contract.

Web projections and other substrates use their own narrow Runtime
adapters. They remain under the same authority model but do not inherit the
Component WIT interface.

The 0.6 tree implements and conformance-tests this Component path. Its shipped
first-party UI Apps remain web projections, so the fixture and authoring
template must not be presented as completed product migration.

The current Component ABI is checked against
[`elastos-bus-v1.wit`](../elastos/wit/elastos-bus-v1.wit). Exact ABI fields,
SDK behavior, role restrictions, and validation rules belong to
[Capsule authoring](CAPSULE_AUTHORING.md). Home launch grants and browser-host
authority belong to the
[Home shell host contract](HOME_SHELL_HOST_CONTRACT.md).

A provider may hold DID signing material only when its declared namespace,
registered identity, and Runtime policy grant that narrow role. The provider
role alone grants nothing. Ordinary capsules instead request typed signing
intents such as `sign_chat_message`; they do not receive arbitrary
`sign(data)` access.

## Authority boundary

Components request effects through typed, capability-secured Bus resources.
Web projections use narrow, capsule-scoped Runtime adapters. Data capsules
carry no execution authority. Provider capsules declare a narrow `provides`
namespace and auditable authority metadata. Operator trust in a provider does
not grant user authority. A provider that needs principal data must use the
corresponding capability path.

The Runtime, Bus, and provider ownership rule is normative in
[PRINCIPLES.md](../PRINCIPLES.md). Trust domains and network compatibility
paths belong to [Architecture](ARCHITECTURE.md) and
[Carrier](CARRIER.md). Current resource names belong to
[Namespaces](NAMESPACES.md). The authority boundary belongs to
[PRINCIPLES.md](../PRINCIPLES.md), [ESP v0](ESP_V0.md), and the
[Capsule interface contract](CAPSULE_INTERFACE_CONTRACT.md).
