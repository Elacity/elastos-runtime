# ElastOS Overview

## What This Repo Is

`elastos-runtime` is the runtime layer of ElastOS.

It provides:

- capsule execution and isolation
- capability issuance and validation
- signed release, install, and update flows
- localhost-rooted state, sharing, and provider routing
- the local trust core for humans and AI

It is not the entire SmartWeb stack. PC2, richer `localhost://` semantics, broader WebSpaces, blockchain/payment integration, and older Android-compatible runtime ideas converge later.

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

Carrier owns networking semantics. Application capsules should consume provider contracts such as `elastos://peer/`, `elastos://ipfs/`, and `elastos://tunnel/` rather than depending on transport details like QUIC, TAP, Kubo, or cloudflared.

## What Works Now

The current preview is grounded in code and recorded proof, but not every path has the same evidence level:

- signed install from `https://elastos.elacitylabs.com/install.sh`
- `elastos setup` for the core PC2 home profile
- `elastos` opens the sovereign PC2 home surface on the current live public `x86_64` line
- `elastos pc2` is the explicit PC2 home alias
- one-terminal native `elastos chat`
- direct `share`, `open`, `shares *`, and `attest` when the explicit extras are installed
- immediate public sharing through `elastos share --public`
- signed publish, install, and update
- native chat as the default proving surface, with explicit WASM and microVM chat proving paths
- initial read-only `webspace-provider` resolution under `localhost://WebSpaces/Elastos`

Current Jetson/WSL front-door re-proof remains open. See [state.md](../state.md) and [RUNTIME_REPO_USER_STORY_CHECKLIST.md](RUNTIME_REPO_USER_STORY_CHECKLIST.md) for the current proof surface.

## Runtime Classes

The current command split is intentional:

- managed dashboard runtime
  - `elastos`
  - `elastos pc2`
- managed user runtime
  - `elastos chat`
- no runtime
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
  - `elastos agent`
  - `elastos capsule`
  - WASM/microVM `elastos run`
- starts own service
  - `elastos serve`
  - `elastos gateway`
  - `elastos site serve`

This keeps the normal user flow simple without silently widening all runtime-backed surfaces. See [COMMAND_MATRIX.md](COMMAND_MATRIX.md).

## Local And Network Spaces

The current rooted-space contract is:

- the user-visible local PC2 namespace is expressed through rooted `localhost://...` paths
- `elastos://` = decentralized identities, peer/provider surfaces, and signed shared content

First-class file-backed localhost roots in the runtime today:

- `localhost://Users/...`
- `localhost://Public/...`
- `localhost://MyWebSite`
- `localhost://Local/...`
- `localhost://UsersAI/...`
- `localhost://AppCapsules/...`
- `localhost://ElastOS/...`
- `localhost://PC2Host/...`

Reserved special root:

- `localhost://WebSpaces/...`
  - this is the future dynamic AppCapsule/WebSpace resolver class, not ordinary path-based storage

Useful current examples:

- `localhost://Users/self/Documents/report.md`
- `localhost://Public/manual.pdf`
- `localhost://MyWebSite`
- `elastos://<cid>`
- `elastos://peer/...`
- `elastos://ai/...`

The current relationship is:

- rooted `localhost://...` paths = the local PC2 namespace
- `localhost://ElastOS/...` = runtime-owned local system state and services
- `elastos://...` = decentralized identities, shared content, and provider-routed surfaces between nodes
- `localhost://WebSpaces/<moniker>/...` = the local mounted/interpreted view of a broader dynamic named space

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
- stable shared identity
  - `elastos://<cid>`
- explicit public exposure
  - local domain
  - ephemeral tunnel
  - supernode / active proxy

## WebSpaces, AppCapsules, and the Object Model

The current target model is:

- **AppCapsules** as the portable app/runtime objects
- **WebSpaces** as named protocol/data spaces interpreted after `://`
- a `localhost`-first user/developer experience where people and agents primarily live inside their own local PC2 world

The longer-term direction is **content-first**: users navigate typed objects (photos, documents, music, models), not application launchers. Capsules act as viewers and editors for object types. The runtime resolves which capsule handles which type. PC2 evolves from "launch apps" to "browse your objects." See [../ROADMAP.md](../ROADMAP.md) for the full native object model direction.

What is already true in code:

- file-backed localhost roots are first-class
- `MyWebSite` and `Public` are distinct
- `http://` is no longer a first-class capability/manifest resource scheme
- an initial read-only `webspace-provider` slice exists for mounted moniker listing/resolution and typed handles under `localhost://WebSpaces/Elastos`
- the current depth boundary is explicit: `content/<cid>` resolves to a file endpoint, while `peer/<id>`, `did/<did>`, and `ai/<backend>` stop at one typed folder handle and fail closed on deeper traversal

What remains open:

- deeper `WebSpaces` daemon/object resolution beyond the initial `Elastos` handle and its first typed children
- stronger root-aware substrate cleanup across the remaining internal tests/examples
- broader system-service mapping and docs cleanup

See [ARCHITECTURE.md](ARCHITECTURE.md), [NAMESPACES.md](NAMESPACES.md), and [state.md](../state.md) for the current direction and proof boundary.

## Humans And AI

Humans and AI agents follow the same capability model.

That means:

- no ambient authority
- explicit capability requests
- scoped access to local and provider resources
- runtime-side validation and audit

What is proven today:

- local operator proof for `elastos agent --backend codex`
- persistent operator Codex chat service on this server

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
- [SITES.md](SITES.md) for the site/public exposure model
- [GLOSSARY.md](GLOSSARY.md) for quick term lookups

Supplemental concept notes, only if you need them:

- [CARRIER.md](CARRIER.md) for the narrower Carrier framing
- [CAPSULE_MODEL.md](CAPSULE_MODEL.md) for the capsule/runtime/object terminology split
