# Glossary

> Supplemental vocabulary note.
>
> This file is for term lookup, not for the primary repo narrative or current
> behavior contract. Use [OVERVIEW.md](OVERVIEW.md) for the system summary and
> [state.md](../state.md), [COMMAND_MATRIX.md](COMMAND_MATRIX.md), and
> [RUNTIME_REPO_USER_STORY_CHECKLIST.md](RUNTIME_REPO_USER_STORY_CHECKLIST.md) for current truth.

Key terms used in the ElastOS codebase and documentation.

**Naming convention:** "ElastOS" (two capitals) is this runtime. "Elastos" is the broader ecosystem and foundation. `elastos` (lowercase) is the binary, crate names, and URI scheme.

## Runtime

The minimal trusted base (`elastos` binary). Enforces isolation, signatures, and capabilities. Everything outside the runtime is a capsule.

## Digital Capsule

The umbrella software or data object model in ElastOS. A Digital Capsule is signed, portable, capability-governed, and explicitly described. It may be an app capsule, provider capsule, shell capsule, agent capsule, or data capsule. See [CAPSULE_MODEL.md](CAPSULE_MODEL.md) for the full model.

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

A capsule with orchestrator capability. The shell decides whether to grant or deny capability requests from other capsules. Can be a CLI, TUI, or web desktop (PC2).

## Provider

A capsule that implements a protocol contract for other capsules to consume. Examples: `localhost-provider` (file-backed localhost roots), `did-provider` (identity), `ai-provider` (LLM routing), `ipfs-provider` (IPFS via Kubo). P2P networking is provided by built-in Carrier, not a separate provider capsule. Application capsules use providers through `elastos://` or rooted `localhost://` resources rather than implementing protocols directly.

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

## WebSpace

In the WCI-aligned model, a WebSpace is a special AppCapsule class that interprets the named data after `://` dynamically. It is not just a folder on disk. The resolver owns the raw moniker first and may then return either a file endpoint or a traversable `folder/` handle. `localhost://WebSpaces/...` is therefore not ordinary local storage; it is the future local handle into named, daemon-resolved spaces such as `Elastos`, `SimpleX.chat`, or `WeChat.com`.
