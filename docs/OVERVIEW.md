# ElastOS Overview

## What This Repo Is

`elastos-runtime` is the runtime layer of ElastOS.

It provides:

- capsule execution and isolation
- capability issuance and validation
- signed release, install, and update flows
- localhost-rooted state, sharing, and provider routing
- the local trust core for humans and AI

It is not the entire SmartWeb stack. Home exists as the current front door, but richer Home object browsing, broader `localhost://` semantics, WebSpaces, blockchain/payment integration, and older Android-compatible runtime ideas converge later.

Read this file as a high-level repo guide. For factual current behavior and proof levels, use [state.md](../state.md), [COMMAND_MATRIX.md](COMMAND_MATRIX.md), and [RUNTIME_REPO_USER_STORY_CHECKLIST.md](RUNTIME_REPO_USER_STORY_CHECKLIST.md).

## Core Direction

The runtime should stay small enough to trust.

Trusted core:

- isolation
- signatures
- capability validation
- local state handling
- provider routing

Everything else should live above that core as capsules, providers, or operator-managed services.

Carrier owns networking semantics. Application capsules should consume product/provider contracts such as `elastos://peer/` and the current `elastos://content/` surface rather than depending on transport details like QUIC, TAP, Kubo, IPFS Cluster, Elacity APIs, or cloudflared. Low-level `elastos://ipfs/` remains a system/provider backend, not the normal app contract.

## What Works Now

The current preview is grounded in code and recorded proof, but not every path has the same evidence level:

- signed install from `https://elastos.elacitylabs.com/install.sh`
- `elastos setup` for the core Home profile
- `elastos setup --profile demo` for the broader demo/test surface, including the hosted chat-room asset set
- `elastos setup --profile operator` for the explicit operator lane
- `elastos` opens Home on the current live public `x86_64` line
- `elastos home` is the explicit Home alias
- passkey-first Home unlock with admin/guest account policy, scoped sessions,
  explicit sign-out, and separate principal roots
- System account, Recovery Kit, Appearance, and advanced runtime/network
  diagnostics without duplicating Wallet approval surfaces
- one Wallet surface for accounts and approval methods, backed by
  `wallet-provider`, connector capsules, and typed Inbox approvals
- typed `chain-provider` status/balance/proof/transaction surfaces without raw
  app-visible node RPC
- a Browser capsule proof that uses the Runtime Browser/Net/Exit/Engine ABI
  instead of host iframe browsing; final product Browser acceptance remains open
- one-terminal native `elastos chat`
- sovereign room invite/accept control plus hosted chat-room access on top of the explicit operator lane
- operator peer control over Carrier with `elastos node info`, `peer`, `status`, `room`, and `update` flows
- direct `share`, `open`, `shares *`, and `attest` when the explicit extras are installed
- immediate public sharing through `elastos share --public`
- signed publish, install, and update
- native chat as the default proving surface, with explicit WASM and microVM chat proving paths
- `webspace-provider` resolution under `localhost://WebSpaces`, with read-only
  resolver mounts plus local materialized writable mounts/forks backed by
  provider-owned object/head tables
- content availability manifests and signed local availability receipts above
  the low-level `ipfs-provider`

Further installed-host front-door re-proof remains open. See [state.md](../state.md) and [RUNTIME_REPO_USER_STORY_CHECKLIST.md](RUNTIME_REPO_USER_STORY_CHECKLIST.md) for the current proof surface.

## Runtime Classes

The current command split is intentional:

- managed dashboard runtime
  - `elastos`
  - `elastos home`
- managed user runtime
  - `elastos chat`
- no runtime
  - `elastos room show`
  - `elastos room pending`
  - `elastos room seed`
  - `elastos room invite-*`
  - `elastos room accept-*`
  - `elastos node info`
  - `elastos node peer *`
  - `elastos share`
  - `elastos open`
  - `elastos shares *`
  - `elastos attest`
  - `elastos update`
  - `elastos setup`
  - `elastos site stage`
  - `elastos site path`
  - `elastos site publish [--release <name>]`
  - `elastos site releases`
  - `elastos site channels`
  - `elastos site activate [--release <name> | --channel <name>]`
  - `elastos site history`
  - `elastos site rollback [release-or-bundle-cid]`
  - `elastos site promote <channel> <release>`
  - `elastos site bind-domain`
- operator runtime
  - `elastos room open`
  - `elastos node status --peer ...`
  - `elastos node room * --peer ...`
  - `elastos node update --peer ...`
  - `elastos agent`
  - non-interactive `elastos capsule`
  - WASM/microVM `elastos run`
- starts own service
  - `elastos serve`
  - `elastos gateway`
  - `elastos site serve`

This keeps the normal user flow simple without silently widening all runtime-backed surfaces. See [COMMAND_MATRIX.md](COMMAND_MATRIX.md).

## Local And Network Spaces

The current rooted-space contract is:

- the user-visible local Home namespace is expressed through rooted `localhost://...` paths
- `elastos://` = decentralized identities, peer/provider surfaces, and signed shared content

First-class file-backed localhost roots in the runtime today:

- `localhost://Users/...`
- `localhost://Public/...`
- `localhost://MyWebSite`
- `localhost://Local/...`
- `localhost://UsersAI/...`
- `localhost://AppCapsules/...`
- `localhost://ElastOS/...`

Reserved special root:

- `localhost://WebSpaces/...`
  - this is the future dynamic AppCapsule/WebSpace resolver class, not ordinary path-based storage

Useful current examples:

- `localhost://ElastOS/Documents/<doc-did>`
- `localhost://Users/<principal-root>/Documents/report.md`
- `localhost://Public/manual.pdf`
- `localhost://MyWebSite`
- `elastos://<cid>`
- `elastos://peer/...`
- `elastos://ai/...`

The current relationship is:

- rooted `localhost://...` paths = the local Home namespace
- `localhost://ElastOS/...` = runtime-owned local system state and services
- `elastos://...` = decentralized identities, shared content, and provider-routed surfaces between nodes
- `localhost://WebSpaces/<moniker>/...` = the local mounted/interpreted view of a broader dynamic named space
- provider-specific targets such as `cloud://drive/...` are resolver-private
  implementation details until a provider contract intentionally exposes them

For documents specifically:

- `localhost://ElastOS/Documents/<doc-did>` = the mutable document object Home and Documents should open
- `localhost://Users/<principal-root>/Documents/<file>.md` = the active passkey principal's working-copy storage path for markdown bytes
- `elastos://<cid>` = an immutable published revision of a document

For browser-facing local sites, the root is:

- `localhost://MyWebSite`

Current reality:

- that root is now implemented as a first-class staged local path
- today the runtime exposes it through `elastos site ...` and `elastos open localhost://MyWebSite`
- `elastos site publish [--release <name>]`, `elastos site releases`, `elastos site promote <channel> <release>`, `elastos site channels`, and `elastos site activate [--release <name> | --channel <name>]` now let users move between editable local roots, friendly named releases, promotion channels, and immutable CID-backed bundles
- local and ephemeral public exposure are explicit operator choices in code

The intended layering is:

- local site root
  - `localhost://MyWebSite`
- stable shared content identity
  - `elastos://<cid>`
- explicit public exposure
  - local domain
  - ephemeral tunnel
  - supernode / active proxy

## WebSpaces, AppCapsules, and the Object Model

The current target model is:

- **AppCapsules** as the portable app/runtime objects
- **WebSpaces** as named protocol/data spaces interpreted after `://`
- a `localhost`-first user/developer experience where people and agents primarily live inside their own local Home world

The longer-term direction is **content-first**: users navigate typed objects (photos, documents, music, models), not application launchers. Capsules act as viewers and editors for object types. The runtime resolves which capsule handles which type. Home evolves from "launch apps" to "browse your objects." See [../ROADMAP.md](../ROADMAP.md) for the full native object model direction.

What is already true in code:

- file-backed localhost roots are first-class
- `MyWebSite` and `Public` are distinct
- `http://` is no longer a first-class capability/manifest resource scheme
- `webspace-provider` exposes mounted moniker listing/resolution, typed handles,
  persistent mount/index/head/object tables, and local materialized write/mkdir/delete
  flows for writable mounts/forks
- the current depth boundary is explicit: `content/<cid>` resolves to a file endpoint, while `peer/<id>`, `did/<did>`, and `ai/<backend>` stop at one typed folder handle and fail closed on deeper traversal

What remains open:

- live external resolver traversal, remote mutable/fork sync, and provider-to-provider
  Carrier invocation beyond the local materialized WebSpace object model
- stronger root-aware substrate cleanup across the remaining internal tests/examples
- broader system-service mapping

See [ARCHITECTURE.md](ARCHITECTURE.md), [NAMESPACES.md](NAMESPACES.md), [CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md), and [state.md](../state.md) for the current direction and proof boundary.
See [DESIGN_SYSTEM.md](DESIGN_SYSTEM.md) for the shared first-party surface palette and human/agent interaction contract.

## Humans And AI

Humans and AI agents follow the same capability model.

That means:

- no ambient authority
- explicit capability requests
- scoped access to local and provider resources
- runtime-side validation and audit
- the same capability-scoped operation behind visible user actions and agent calls

What is proven today:

- local operator proof for the explicit `elastos agent` path
- the explicit operator lane as a real, separate surface from Home

What is not claimed today:

- a packaged end-user AI workflow
- vendor-shaped AI namespaces as the final public architecture

## Sharing Contract

`elastos share` gives a canonical reference first:

- `elastos://<cid>` = stable identity
- `elastos open elastos://<cid>` = local preview path
- `elastos share --public` = immediate public edge while the command runs

Gateway URLs are convenience transport only. They may take time to propagate and should not be treated as the canonical identity.

## Where To Read Next

- [state.md](../state.md) for factual current state
- [GETTING_STARTED.md](GETTING_STARTED.md) for install and source flows
- [ARCHITECTURE.md](ARCHITECTURE.md) for the full technical design
- [CONTENT_AVAILABILITY.md](CONTENT_AVAILABILITY.md) for IPLD, CID sync, availability receipts, and the SmartWeb content-plane direction
- [SITES.md](SITES.md) for the site/public exposure model
- [GLOSSARY.md](GLOSSARY.md) for quick term lookups

Supplemental concept notes, only if you need them:

- [CARRIER.md](CARRIER.md) for the narrower Carrier framing
- [CAPSULE_MODEL.md](CAPSULE_MODEL.md) for the capsule/runtime/object terminology split
