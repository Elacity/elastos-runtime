# Digital Capsule model

[GLOSSARY.md](GLOSSARY.md) defines the canonical terms, and
[CAPSULE_AUTHORING.md](CAPSULE_AUTHORING.md) defines
manifest fields and supported combinations.

For system context, see the [repository README](../README.md) and
[ARCHITECTURE.md](ARCHITECTURE.md). [state.md](../state.md) records verified
implementation status, and [TASKS.md](../TASKS.md) records open work.

## Core model

A Digital Capsule is a portable, signed software or content package
with an identity, interface, and lifecycle. Runtime admits and runs executable
capsules. Carrier handles endpoint-authenticated peer and content transport
when Runtime selects an off-box route.

Every independently installable executable or sealed-content package is a
capsule. Browser UI, Browser Engine, Exit, Assistant, Builder, model providers,
and viewers are executable or provider capsules. Models, games, protected
media, and published document bundles are passive content capsules.

Files that implement one package belong to that package closure. A Chromium
binary, VM image, native library, model weight, or other internal dependency
does not become a separate Marketplace item merely because it is a separate
file. Mutable user state, signed heads, catalog entries, availability receipts,
admission records, rights evidence, grants, and service offers describe a
capsule or object. They are not capsules themselves.

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

## Package and launch closures

ElastOS uses two related closures.

The **package closure** contains the canonical manifest and every immutable file
needed by one package. Its root CID identifies the exact bytes. The publisher
signs an assertion that binds its identity and provenance to that CID; the
signature is not a second package identity. Changing any package file or pinned
internal dependency creates a new CID.

The **resolved launch closure** records what one instance actually uses. It
binds the app or content CID, exact local provider and native-variant CIDs,
approved remote service offers, Runtime grants, mutable-state revision, host
measurements, principal, device, and session. An external interface requirement
can resolve to a compatible local provider or approved remote service without
changing the requesting capsule's CID. Runtime records the selected bindings
before effects.

Every executable file loaded for a local provider, including dynamically loaded
native libraries, must be part of a verified package closure or a verified host
measurement covered by the launch record. A remote provider remains outside the
consumer's package closure and is bound through its signed service offer,
authenticated endpoint, grant, and declared interface.

## Canonical record graph

Each fact has one owning record. User interfaces and catalogs project these
records; they do not create competing truth.

| Fact | Authoritative record |
| --- | --- |
| Exact package bytes | Immutable capsule closure CID |
| Publisher provenance | Signed assertion binding publisher DID to that CID |
| Stable app or object identity | Signed mutable head, optionally controlled through EID or another verified registry |
| Current byte retention | Content-owned signed availability receipt |
| Local installation | Runtime admission record for the verified closure |
| Entitlement to use, buy, rent, export, or decrypt | Versioned rights-provider evidence, such as a grant, lease, license, or token proof |
| Authority for one local effect | Runtime capability issued after the applicable evidence and policy checks |
| Running capability | Capsule instance or signed remote service offer and grant |
| Mutable user data | Principal- or workspace-owned object revision and signed head |

A catalog entry points to a package CID and supplies bounded presentation and
policy facts. An availability receipt states which provider currently accepts
retention responsibility. An admission record states what one Runtime verified
and accepted. A service offer describes a running capability. A listing, NFT,
or access token supplies market or rights evidence. None of these records
replaces the package CID.

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

## Lifecycle and Marketplace actions

Marketplace is a view and workflow over Runtime records. It does not own a
parallel package database. Home, Marketplace, Library, System, CLI, and agent
surfaces must preserve these action meanings:

The shared lifecycle is package and sign, publish and discover, fetch and
verify, admit, resolve and authorize, instantiate and use, then stop, update, or
remove. Passive content skips execution but keeps the same identity,
availability, admission, rights, state, and removal records.

| Action | Required result |
| --- | --- |
| Get | Fetch, verify, and atomically admit the exact capsule CID. |
| Keep | Request continued local or remote retention under an explicit policy. |
| Buy | Acquire defined rights. Content may already be retained or admitted. |
| Open or Use | Authorize and create an instance with compatible providers and state. |
| List | Publish a package or rights listing without changing package identity. |
| Offer service | Advertise a running service to eligible principals. |
| Update | Move a trusted signed head to a new capsule CID and run any explicit state migration. |
| Remove | Choose package removal, retention release, state deletion, or rights retirement as separate effects. |

A product flow may compose actions, but Runtime keeps their results and receipts
separate. Buying does not imply retention. Retention does not grant use or
decryption. A service grant does not grant package installation, local storage,
sandbox, wallet, or tool authority.

## Capability placement

An app composes typed capabilities whose execution and state can have different
placements. Runtime acquires verified capsules for local execution or binds
approved remote services. It checks interfaces, host support, resources, rights,
and authority before effects. Package availability, execution grants, and state
access remain separate decisions. Capsule code uses the same contract across
placements.

A local Browser UI can use a remote Chromium VM Engine, a local Exit, and local
durable state. A local Assistant or Builder can use remote inference, a local
sandbox, and local durable state. Remote inference gains no local tool authority;
Runtime authorizes each requested local action independently. Each advertised
combination needs compatible providers and installed proof.

When execution and durable storage use different nodes, the state owner retains
the canonical checkpoint and grants the execution host a bounded working copy.
The execution host returns revisions through the common object and protection
path. State transfer does not grant unrelated execution or storage authority.

## Identity, rights, protection, and availability

The local Runtime principal remains the authority subject. A passkey proves the
local account, a Profile DID identifies a person or contact, a Device DID
identifies a node and Carrier endpoint, and a publisher DID signs package
claims. EID can add global identity, controller keys, credentials, recovery
links, service endpoints, and rights evidence. Runtime verifies that evidence
and converts it into a scoped local capability. EID does not replace the local
principal, package CID, or Runtime decision.

A chain-backed Icon NFT can control a stable application or rights record whose
signed head resolves to an exact capsule CID. The manifest `icon` field remains
presentation artwork. Marketplace chain formats belong behind a versioned
rights adapter so token or contract changes do not enter the Runtime core.
Already admitted public, private, or permanently purchased content can use
cached signed evidence offline. Revocable rights declare a freshness window,
trusted-time requirement, and offline-expiry behavior.

Encrypted objects use the same content and availability path as public objects.
IPLD defines their graph, Content owns retrieval and retention, Carrier supplies
authenticated transport, and IPFS, CAR, erasure-coded shards, or another
storage backend remains behind the selected provider. A holder can retain
ciphertext without receiving decryption or execution authority.

Protection policy selects one of these explicit modes:

- Private owner data keeps keys under the owner's protection and recovery policy.
- A permanent purchase seals the content key to the buyer. It can work offline,
  but delivery of the usable key cannot provide later revocation.
- Revocable sharing or rental uses a bounded lease, dKMS, or attested decrypt
  provider. Offline access ends when its accepted proof expires.
- Exclusive resale or one-time redemption can use hardware-backed custody while
  the usable key remains inside that boundary. After disclosure, deletion by a
  prior holder cannot be proved; exclusivity then relies on explicit trust or
  legal terms.

Storage replication and its storage-payment settlement policy are
availability-provider concerns. An erasure-coded mesh may reconstruct the same
ciphertext and prove shards against
the package graph; it does not create a new capsule identity or a second
consumer acquisition protocol.

A hardware dongle can hold EID and wallet keys, verify the package and resolved
launch closures, authorize key release, and sign launch evidence. Protecting
decrypted code or data from a hostile execution host also requires an attested
execution environment on that host. The dongle is a proof and key provider; it
does not replace Runtime capabilities, Content availability, or instance
isolation.

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

Current implementation and product migration evidence are recorded in
[state.md](../state.md). A fixture or authoring template alone is not product
acceptance.

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

## Conformance invariants

One common acceptance contract applies to apps, providers, models, games, and
protected media. Type-specific preparation, playback, and rights checks extend
it; they do not replace it.

- The same complete closure always has the same CID. Changing any covered byte
  or dependency changes the CID, and a missing or altered member fails before
  execution or admission.
- Publisher provenance, package identity, availability, local admission,
  rights, execution readiness, and mutable state remain separately observable.
- Two authorized holders can independently serve the same CID. A stale or
  failed holder cannot prevent a later valid holder within the bounded attempt.
- Interrupted acquisition never creates partial admission. Retry, cancellation,
  restart reuse, tamper rejection, and removal have explicit terminal results.
- Runtime selects compatible local providers or approved remote services through
  the same declared interface and records the resulting launch closure.
- Package update preserves principal-owned state unless an explicit migration
  succeeds. Package removal, retention release, rights revocation, state
  deletion, and key retirement remain separate operations.
- Offline behavior follows the selected protection policy. Permanent key
  delivery cannot claim revocation, and revocable access cannot claim unlimited
  offline use.
- Capsules receive typed Runtime resources. Storage, chain, wallet, IPFS, host,
  and Carrier implementation details remain outside capsule authority.
