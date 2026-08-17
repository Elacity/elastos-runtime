# Glossary

> Supplemental vocabulary note.
>
> This file is for term lookup, not for the primary repo narrative or current
> behavior contract. Use [ARCHITECTURE.md](ARCHITECTURE.md) for the system
> summary and [state.md](../state.md), [COMMAND_MATRIX.md](COMMAND_MATRIX.md), and
> [RUNTIME_REPO_USER_STORY_CHECKLIST.md](RUNTIME_REPO_USER_STORY_CHECKLIST.md) for current truth.

Key terms used in the ElastOS codebase and documentation.

**Naming convention:** "ElastOS" (two capitals) is this runtime. "Elastos" is the broader ecosystem and foundation. `elastos` (lowercase) is the binary, crate names, and URI scheme.

## ElastOS Four Quadrants

The planning frame for balancing the system: **PC2/Home**, **Runtime**,
**Carrier**, and **Blockchain**. The quadrants are responsibility boundaries, not
separate products. See [ARCHITECTURE.md](ARCHITECTURE.md#elastos-four-quadrants)
for the canonical definition.

## Runtime

The minimal trusted base (`elastos` binary). Enforces isolation, signatures, and capabilities. Everything outside the runtime is a capsule.

## Principal

The runtime authority subject. A principal may represent a human account,
agent, device, capsule, or provider. Sessions and capability tokens are issued
to principals. Wallet addresses, passkeys, and DIDs are proof bindings or linked
identities for a principal, not replacements for the principal itself.

## Passkey

The default human proof for Home. A passkey proves control of a local account to
the runtime and unlocks short-lived sessions. It is not a wallet, not a DID, and
normally does not expose raw key material for encryption.

## Profile DID

The stable person/contact identity used by signed People, discovery, accepted
contacts, and direct-conversation identity. A Profile DID belongs to a
principal-owned signed Profile document and remains stable across device
replacement or revocation. It is not the passkey and not the local principal
identifier.

## WebAuthn PRF

A WebAuthn extension that can derive credential-bound secret output during a
passkey ceremony. In ElastOS this may become a passkey-root protector for
wrapping a principal data key on the client side. Raw PRF output is key material:
it must not be sent to runtime auth routes, logged, or stored as ordinary
session data.

## Device DID (`did:key`)

The self-certifying device/node identity derived from local key material. The
runtime currently uses `did:key` for Carrier/node signing, routing, delivery
attribution, and local provider identity. It is useful without a blockchain,
but it is device/transport identity, not person/contact identity, and it does
not prove a global name claim.

## Account DID (`did:elastos` / EID)

A future or linked global account identity anchored by Elastos DID/EID
infrastructure. Use it for portable profiles, credentials, publisher identity,
service endpoints, recovery, DAO actions, and globally unique name claims. It
is not required for a local passkey account.

## Handle / Name

A human-readable label such as `alice`. Local handles are display names scoped
to one runtime or space and can collide elsewhere. A globally scarce name needs
a registry with consensus, ownership, transfer, recovery, and expiry rules, such
as an EID/DID-chain namespace or a namespace smart contract. Runtime authority
must bind to principal IDs, DIDs, signatures, and CIDs, not to an unverified
handle string.

## Digital Capsule

The portable signed package model in ElastOS. A Digital Capsule is capability-governed and explicitly described. It may be an app capsule, provider capsule, shell capsule, agent capsule, or sealed data/content capsule. User objects such as documents remain first-class objects; they become data capsules only when packaged with capsule metadata and provenance. See [CAPSULE_MODEL.md](CAPSULE_MODEL.md) for the full model.

## Capsule

Shorthand for a Digital Capsule, usually referring to an executable one. Capsules start with zero ambient authority and must request capability tokens for any action. Two main executable substrates exist today: **WASM** (lightweight) and **microVM** (full Linux sandbox via crosvm).

## Capsule Runtime (AppCapsule Runtime)

The per-capsule execution contract. This is the common runtime surface that makes one capsule portable across substrates such as WASM and microVM. It is not the trusted node core and not Carrier. In the current repo, this concept spans `elastos-guest`, `elastos-compute`, `elastos-crosvm`, and the guest bridge protocols rather than one single crate.

## Capsule Artifact

The immutable packaged form of a capsule: manifest, code or rootfs payload, and signature/provenance material.

## Capsule Instance

One running copy of a capsule, bound to a session, capability set, and execution substrate.

## Capsule State

Mutable state associated with a capsule instance or user, kept separate from the immutable capsule artifact.

## Shell

A capsule that presents Runtime facts and emits typed user or agent intents. A
shell may be graphical, CLI, or TUI, but its manifest role is not an authority
grant. Runtime policy decides which shell identities may become active and
authorizes every launch, approval, provider, and shell-switch effect.

## Provider

A capsule or Runtime-owned service that implements a typed contract. Examples
include `localhost-provider` for rooted local storage, `did-provider` for
identity, `chain-provider` for typed chain reads and proofs, `wallet-provider`
for Wallet proof and approval authority, `rights-provider` for protected-content
rights evidence, `decrypt-provider` for scoped decrypt/render sessions,
`availability-provider` for configured replication, and `ipfs-provider` for
low-level local IPFS through Kubo. Runtime selects providers. Application
capsules use typed Runtime resources instead of choosing providers or network
routes. P2P networking is provided by Carrier below Runtime routing.

## Content Availability Provider

The intended higher-level provider contract for publishing, fetching, checking,
repairing, and unpublishing SmartWeb objects. It sits above low-level
`ipfs-provider` and hides whether bytes are pinned locally, replicated through
Elacity/supernodes, served by volunteer nodes, or later backed by paid storage
markets. Normal app capsules should use `elastos://content/*`, not raw
`elastos://ipfs/*`.

## Protected Content Coordination

The intended Runtime-owned sequence for sealed content. Runtime will bind the
exact object, Profile, Wallet-approved action, session, rights evidence, custody
epoch, and decrypt session. It will then coordinate rights, custody, and decrypt
providers.
Apps receive scoped output or an opaque handle. They do not receive raw CEKs,
custody shares, provider routes, endpoint DIDs, network locations, credentials,
Wallet RPC, Chain RPC, Kubo/IPFS APIs, or Elacity authority.

## dKMS

Distributed key-management system for protected content. The ElastOS direction is
PQ-hybrid threshold release for new content: AES-256 CEKs, `t-of-n` shares,
hybrid X25519 + ML-KEM share wrapping, Runtime-selected custody providers, and
scoped decrypt sessions.
FROST may help sign classical v0 receipts or cohort decisions, but it is not the
long-term dKMS security root.

## Carrier

The authenticated off-box communication and content transport of an ElastOS
node. Carrier can carry peer discovery, messaging, streams, replication, and
content transfer when Runtime routing selects it. It is not the capsule API,
the authority system, or the whole Runtime. Transport-peer authentication does
not by itself prove application-message authorship. See
[CARRIER.md](CARRIER.md) for the full framing.

## `elastos://`

The native namespace exposed by the runtime. It is broader than Carrier alone:

- some `elastos://` operations are Carrier-backed, such as peer and content flows
- some are provided by first-party providers, such as DID or AI routes
- all are routed and capability-checked by the runtime

So `elastos://` is the contract surface; Carrier is one major substrate behind it.

## HTTP

HTTP is an implementation and compatibility protocol in this repo, not the definition of Carrier.

Main roles:

- runtime control API between capsules/shell and the node
- browser/gateway access path for humans and installers
- tunnel/edge exposure for web interoperability

Trust still comes from capabilities, hashes, and signatures, not from HTTP itself.

## Guest Network

An explicit compatibility mode where a capsule gets conventional guest
networking instead of using its typed Runtime resources and substrate adapter.
This is useful for provider capsules or legacy workloads that truly need raw
TCP or guest-facing services, but it is not the preferred default for ordinary
Apps.

## iroh

The current transport implementation for Carrier's network plane. A Rust library providing QUIC, DHT-based peer discovery, gossip messaging, and mDNS local discovery. Used by the built-in Carrier node (`carrier.rs`).

## Boson / Carrier Native

The Elastos Foundation's native Carrier protocol. A future transport target for interoperability — when Boson matures, it becomes another transport under the Carrier abstraction alongside iroh.

## TAP Device

A virtual network interface used only when a microVM capsule is explicitly
placed into guest-network compatibility mode. It provides an isolated
point-to-point link between the VM and the host. Ordinary Apps use their typed
Runtime resources through the selected substrate adapter and should not require
TAP or `sudo` at launch. TAP remains a Runtime-owned compatibility path for
workloads that genuinely need a guest NIC.

## Capability Token

A cryptographically signed permission (Ed25519). Grants a specific capsule the right to perform a specific action on a specific resource. Tokens have constraints: epoch, expiry, max uses, delegatability.

## Capability View

A derived per-capsule projection of active Runtime grants, provider bindings,
and WebSpace mounts. It helps shells, Capsules, and System inspection discover
which resources are currently available under stable names. The view grants no
authority by itself: every resource operation still requires the applicable
Runtime-verified capability. It is not a second token store or namespace
access-control system.

## CID (Content ID)

A content-addressed identifier (hash of the content). Used for capsule identity, IPFS references, and the `elastos://` namespace. The identity is the content, not the location.

CID is not an availability guarantee. A CID says what bytes or graph root are
being referenced; availability receipts say which provider accepted or verified
responsibility to keep that content reachable.

A CID is also not a person, device, account, or global name claim. A stable
object may later have a signed head or object DID that points to changing CID
revisions, but each CID already identifies one immutable byte graph.

## IPLD

InterPlanetary Linked Data: the content-addressed object graph/data model used
to represent hash-linked structures. In ElastOS it is the right model for
published object manifests, signed heads, provenance records, sealed-content
descriptors, and availability receipts. IPLD is not Carrier, not IPFS storage,
and not an access-control or rights system.

## WebSpace

In the World Computer-aligned model, a WebSpace is a mounted resolver surface for
named data and provider-backed resources. It is not just a folder on disk.
`localhost://...` is the operator's own local computer space. Domain/DNS-backed
spaces such as `joe.ela.city://...` or `dns://team-space/...` are other
principals' SmartWeb spaces, mounted only through explicit capability keys.
The resolver owns `localhost://WebSpaces/<mount>/...` first and returns typed
handles instead of letting an app walk raw storage. A mount can project native
ElastOS resources such as `elastos://content/*`, named remote spaces, or
resolver-private provider targets such as `cloud://drive/...`; those targets
remain provider authority, while Library/Home show the mounted WebSpace view.
Mutable WebSpace mounts/forks can also materialize local provider-owned objects
with explicit access-policy metadata; that local materialization is provider
state, not a raw app-visible filesystem alias.
