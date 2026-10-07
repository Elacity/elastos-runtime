# Digital Capsule model

[GLOSSARY.md](GLOSSARY.md) defines the canonical terms, and
[CAPSULE_AUTHORING.md](CAPSULE_AUTHORING.md) defines
manifest fields and supported combinations.

For system context, see the [repository README](../README.md) and
[ARCHITECTURE.md](ARCHITECTURE.md). [GitHub issues](https://github.com/Elacity/elastos-runtime/issues) own status, acceptance and proof.

## Current isolation and target boundary

First-party apps run as web projections in the browser's opaque sandboxed
frames. Runtime checks each app's signed launch token and actor before it performs an effect.
Home can currently obtain every app's capability, so a compromised Home can
reach those apps' authority. The target limits Home to delegation and gives each
app a separate, revocable capability. The WASM Component authoring path runs in
Wasmtime with memory and fuel limits and Runtime Bus hostcalls.

Providers run as native operating-system processes with the Runtime user's
rights. Only the model provider is partly confined. The trusted shell helper
also runs as a native host process. The web Terminal is disabled by default;
host developer mode and closed guest registration are required to enable it.
An enabled Terminal runs commands with the host user's rights.

The seed operator can read hosted data, wallet keys and recovery material.
Passkeys control sign-in; stored data and keys remain accessible to the Runtime
account and root while Home is locked. Protection against hosted operators and
root, and against other software or OS users while a self-hosted Home is locked,
is the target of [hosted protection](https://github.com/Elacity/elastos-runtime/issues/209)
and [locked Home protection](https://github.com/Elacity/elastos-runtime/issues/210).
An unlocked self-hosted Home trusts its owner and their host software. Recovery
from a stolen device or profile key requires a new identity.

The [isolation plan](https://github.com/Elacity/elastos-runtime/issues/173)
records the remaining gates. Source checks describe this source tree. Accepted
installed proof binds the exact Runtime, components and app assets to the
journeys tested on that device.

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
| Artifact | Immutable manifest-and-payload closure, normally named by content ID. A valid signature proves control of its signing key; Runtime publisher policy establishes publisher trust. Provenance records describe claimed lineage. |
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

## Capability placement

An app composes typed capabilities whose execution and state can have different
placements. Runtime acquires verified capsules for local execution or binds
approved remote services. It checks interfaces, host support, resources and
authority before effects, preserves explicit selections and explains missing
requirements. Package availability, execution grants and state access remain
separate decisions. Capsule code uses the same contract across placements.

A local Browser UI can use a remote Chromium VM Engine, a local Exit and local
durable state. A local Assistant or Builder can use remote inference, a local
sandbox and local durable state. The inference result proposes local actions;
Runtime authorises each tool operation independently. Each advertised combination
needs compatible providers and installed proof. A provider manifest or a model
reply alone does not establish working sandbox support.

The [shared state contract](STORAGE_AND_ACCESS.md#shared-application-state)
defines checkpoints and handoff when execution and durable storage are on
different nodes. [Release acceptance](https://github.com/Elacity/elastos-runtime/issues/93) owns placement evidence;
broad placement support is a target, not a claim of hardware qualification.

## Isolation boundary

Runtime admits an executable artifact for a session and binds the instance to
declared resources and capabilities. User-scoped authority also requires a
verified principal. The target gives ordinary instances access to mutable state
through capability-scoped object and WebSpace contracts. The current web-frame
and native-provider boundaries are described above.

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

The Component fixture and authoring template test this contract. Product App
migration requires its own [isolation evidence](https://github.com/Elacity/elastos-runtime/issues/173).

The current Component ABI is checked against
[`elastos-bus-v1.wit`](../elastos/wit/elastos-bus-v1.wit). Exact ABI fields,
SDK behavior, role restrictions, and validation rules belong to
[Capsule authoring](CAPSULE_AUTHORING.md). Home launch grants and browser-host
authority belong to the
[Home shell host contract](HOME_SHELL_HOST_CONTRACT.md).

The target permits a provider to hold DID signing material only when its declared
namespace,
registered identity, and Runtime policy grant that narrow role. The provider
role alone grants nothing. Ordinary capsules instead request typed signing
intents such as `sign_chat_message`; they do not receive arbitrary
`sign(data)` access.

## Authority boundary

Components request effects through typed, capability-secured Bus resources.
Web projections use narrow, capsule-scoped Runtime adapters. Data capsules
carry no execution authority. Provider capsules declare a narrow `provides`
namespace and auditable authority metadata. The target separates operator trust
from user authority. A provider that needs
principal data uses the corresponding capability path. Current native
processes can retain host-account access beyond that declaration.

The Runtime, Bus, and provider ownership rule is normative in
[PRINCIPLES.md](../PRINCIPLES.md). Trust domains and network compatibility
paths belong to [Architecture](ARCHITECTURE.md) and
[Carrier](CARRIER.md). Current resource names belong to
[Namespaces](NAMESPACES.md). The authority boundary belongs to
[PRINCIPLES.md](../PRINCIPLES.md), [ESP v0](ESP_V0.md), and the
[Capsule interface contract](CAPSULE_INTERFACE_CONTRACT.md).
