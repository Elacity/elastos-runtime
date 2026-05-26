# Glossary

> Supplemental vocabulary note.
>
> This file is for term lookup, not for the primary repo narrative or current
> behavior contract. Use [OVERVIEW.md](OVERVIEW.md) for the system summary and
> [state.md](../state.md), [COMMAND_MATRIX.md](COMMAND_MATRIX.md), and
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

## WebAuthn PRF

A WebAuthn extension that can derive credential-bound secret output during a
passkey ceremony. In ElastOS this may become a passkey-root protector for
wrapping a principal data key on the client side. Raw PRF output is key material:
it must not be sent to runtime auth routes, logged, or stored as ordinary
session data.

## Device DID (`did:key`)

The self-certifying device/node identity derived from local key material. The
runtime currently uses `did:key` for Carrier/node signing and local provider
identity. It is useful without a blockchain, but it does not prove a global
name claim.

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

A capsule with orchestrator capability. The shell decides whether to grant or deny capability requests from other capsules. Can be a CLI, TUI, or Home surface.

## Provider

A capsule that implements a protocol contract for other capsules to consume. Examples: `localhost-provider` (file-backed localhost roots), `did-provider` (identity), `ai-provider` (LLM routing), `chain-provider` (typed chain reads/proofs), `wallet-provider` (wallet proof, account-link, and approval authority), `drm-provider` (protected-content open boundary), `rights-provider` (protected-content rights questions), `key-provider` (protected-content key release), `decrypt-provider` (protected-content decrypt/render sessions), `availability-provider` (configured replication adapter), and `ipfs-provider` (low-level IPFS via Kubo). P2P networking is provided by built-in Carrier, not a separate provider capsule. Application capsules use providers through `elastos://` or rooted `localhost://` resources rather than implementing protocols directly.

## Content Availability Provider

The intended higher-level provider contract for publishing, fetching, checking,
repairing, and unpublishing SmartWeb objects. It sits above low-level
`ipfs-provider` and hides whether bytes are pinned locally, replicated through
Elacity/supernodes, served by volunteer nodes, or later backed by paid storage
markets. Normal app capsules should use `elastos://content/*`, not raw
`elastos://ipfs/*`.

## Protected Content Provider

The Runtime-mediated `elastos://drm/*` contract for sealed/protected content.
It validates open requests and delegates rights, key release, and decrypt/render
work to provider-owned authority. Apps do not receive raw CEKs, Lit SDKs, wallet
RPC, chain RPC, Kubo/IPFS APIs, or Elacity credentials.

## dKMS

Distributed key-management system for protected content. The ElastOS direction is
PQ-hybrid threshold release for new content: AES-256 CEKs, `t-of-n` shares,
hybrid X25519 + ML-KEM share wrapping, and provider-owned key/decrypt sessions.
FROST may help sign classical v0 receipts or cohort decisions, but it is not the
long-term dKMS security root.

## Carrier

The decentralized communication and content substrate of an ElastOS node. Carrier covers peer discovery, messaging, relay, and peer-to-peer content transfer for native `elastos://` operations. Carrier is hosted by the runtime, but it is not identical to the whole runtime or control plane. See [CARRIER.md](CARRIER.md) for the full framing.

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

An explicit compatibility mode where a capsule gets conventional guest networking instead of relying only on the Capsule Runtime bridge and Carrier/provider calls. This is useful for provider capsules or legacy workloads that truly need raw TCP or guest-facing services, but it is not the preferred default for normal app capsules.

## iroh

The current transport implementation for Carrier's network plane. A Rust library providing QUIC, DHT-based peer discovery, gossip messaging, and mDNS local discovery. Used by the built-in Carrier node (`carrier.rs`).

## Boson / Carrier Native

The Elastos Foundation's native Carrier protocol. A future transport target for interoperability — when Boson matures, it becomes another transport under the Carrier abstraction alongside iroh.

## TAP Device

A virtual network interface used only when a microVM capsule is explicitly placed into guest-network compatibility mode. It provides an isolated point-to-point link between the VM and the host. Normal app capsules use the Carrier-only serial bridge model and should not require TAP or sudo at launch. TAP remains a runtime-owned escape hatch for workloads that still need a real guest NIC and is currently managed by `elastos-crosvm/network.rs`.

## Capability Token

A cryptographically signed permission (Ed25519). Grants a specific capsule the right to perform a specific action on a specific resource. Tokens have constraints: epoch, expiry, max uses, delegatability.

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

In the WCI-aligned model, a WebSpace is a special AppCapsule class that interprets the named data after `://` dynamically. It is not just a folder on disk. The resolver owns the raw moniker first and may then return either a file endpoint or a traversable `folder/` handle. `localhost://WebSpaces/...` is therefore not ordinary local storage; it is the future local handle into named, daemon-resolved spaces such as `Elastos`, `SimpleX.chat`, or `WeChat.com`.
