# Capsule Interface Contract

This is the shared surface that GUI shells, CLI shells, System, and future
Carrier transports consume. It is a projection contract, not an authority layer.

The contract is:

`capsule manifest -> Runtime facts -> shell projection -> typed intent -> Runtime gate -> provider/Carrier effect`

## Surfaces

Every capsule should be understood through these surfaces:

| Surface | Current source of truth | Rule |
| --- | --- | --- |
| Web projection | Home launch targets from `/api/apps/home/summary` and `/api/apps/home/launch` | A web view is a projection. Launch authority comes from Runtime tokens. |
| CLI projection | `home-cli` commands over Home, catalog, interface, service, approval, and ESP facts | CLI renders the same facts as GUI, emits host intents, and can attempt runtime-policy affordance invocation through the Runtime route. It does not bypass providers. |
| Facts | `/api/capsules/catalog`, `/api/capsules/interfaces`, `/api/esp/initialize` | Facts are read-only projections and must tolerate unknown fields. |
| Contract audit | `/api/capsules/contracts/audit` | A System launch token exposes Runtime's join of installed first-party manifests, live launch targets, provider registrations, Carrier availability, viewer relationships, and the generic affordance dispatcher. A non-empty error set returns HTTP 409. |
| Affordances | Manifest-declared `interfaces[*].methods` projected through `/api/capsules/interfaces` | Affordances describe possible calls. They are not grants. |
| Viewer bindings | Manifest `viewer` on `role=content` capsules, projected as `viewer`, `viewer_title`, and viewer-side `accepted_content` in `/api/capsules/catalog` and Home targets; viewer `input_schema.accepts` for Library object compatibility | Content-to-viewer compatibility is a Runtime fact. Viewers such as GBA Emulator do not guess; they accept content capsules that declare them as viewer. Viewers such as Documents and Archive also declare the Library object shapes they can open. |
| Gate metadata | ESP verbs, method risk/gate descriptors, Runtime route policy, and Inspector gate preview | Route-specific Runtime gates decide authority. |
| Audit / mirror view | Catalog trust/provenance fields plus System Inspector mirrors | Ordinary shells get redacted facts. Privileged mirrors stay System/Runtime-owned. |

`/api/capsules/catalog` also derives a `projection` object for each capsule
using schema `elastos.capsule.projection/v1`. This is not another manifest
surface. It is Runtime's compact shell-facing status for web, CLI, facts,
affordances, gates, audit/mirror, and Carrier/service readiness, derived from
the manifest, launch targets, provider namespace, capabilities, and trust
projection that Runtime already knows.

## Trust And Authority

Trust material, verification evidence, declared permissions, executable
bindings, and policy gates are independent facts. A signature or verification
result does not authorize a capsule and does not make an affordance executable.
Manifest risk is advisory metadata, not a grant or denial. Missing evidence is
unknown and must never be presented as safe. Routes, frames, iframe placement,
same-origin access, and successful HTTP responses are transport or presentation
details; only Runtime tokens, concrete bindings, route policy, provider gates,
and approval state can authorize an effect.

## Shell Rules

- `home-gui` and `home-cli` are sibling shell capsules with the same trust
  class, opaque-frame isolation, Runtime facts, launch validation, lifecycle, and
  common host intents.
- `home-gui` renders desktop, windows, launcher, taskbar, and app chrome from
  its opaque sandboxed frame.
- `home-cli` renders commands, context, capsule facts, affordances, gates, and
  approval hints. It lists and accepts generic invoke commands only when
  Runtime reports `bindings[*].executable=true`; other methods remain readable
  descriptions or move through their owning capability/approval surface.
- Shells may ask Home to open a visible capsule with a typed host intent.
- Shell changes are separate explicit intents. Running a normal CLI command
  must not switch to `home-gui`; opening a GUI-only projection from CLI must be
  presented and authorized as `switch shell and open`.
- Shells must not call providers directly for authority-bearing effects.
- Shells must not use ambient same-origin state to switch shells or dispatch
  provider operations.
- Ordinary browser capsule projections use opaque sandboxed origins on the
  existing Home hostname. They never receive `allow-same-origin`, Home's
  ambient session, or DOM access.
- Persistent capsule state belongs in principal-scoped Runtime storage. Browser
  storage is not an authority or durability boundary.
- Entry documents and assets permit loading inside the opaque Home frame.
- Shells must ignore unknown fact fields.

## Current Intent Path

Browser-hosted shells and capsule projections use source-, origin-, target-,
and token-checked host messages such as `home:launch-target`,
`home:open-target`, `home:close-self`, and `home:refresh-summary`. The Home host
accepts common shell lifecycle and launch requests from either `home-gui` or
`home-cli`; projection-specific presentation remains inside the selected shell.

Those messages are local adapter details. The durable model is:

`shell capsule -> Runtime capability/session -> ESP facts/intents -> Runtime gate -> provider/Carrier plane`

Future Carrier transport must preserve the same schemas, gates, consent path,
dispatch path, and audit semantics.

## Invocation Bindings

Every declared method remains visible in `/api/capsules/interfaces`, alongside
one Runtime-derived binding record. A manifest declaration alone never makes a
method callable.

- `executable` means the generic Runtime invoke route has a concrete handler and
  current policy permits the call.
- `approval-required` means a handler exists but the generic route cannot
  satisfy the required approval.
- `provider-path-only` means a live provider and canonical Runtime action exist,
  but the effect must use the capability-gated provider/Carrier path.
- `descriptive-only`, `handler-unavailable`, and `unbound` are non-executable.

`/api/capsules/interfaces/invoke` dispatches only Runtime bindings. It does not
call `ProviderRegistry` directly. Provider operations retain their existing
capability token, action mapping, approval, Carrier routing, and audit path.
Home CLI uses the same binding records for both `invoke list` and command
acceptance, and fails closed when a record is absent.

## Product And Development Inventory

`/api/capsules/catalog`, `/api/capsules/interfaces`, Home launch targets, System,
Marketplace, and Home CLI use one product inventory: a capsule must have a valid
manifest in the installed Runtime capsule tree and its name must be active in
the installed `components.json`. Missing or invalid `components.json` fails
closed. Checked-in source directories never make a capsule installed,
launchable, or invokable.

This endpoint is the installed product inventory, not a global network catalog.
A future Home Get projection may list signed content-capsule entries that are
not yet installed, but each row must point to the canonical signed manifest and
bundle CID. Runtime owns the typed Get operation, package verification, atomic
admission, receipt, and inventory refresh. The projection must not call setup or
download helpers directly or keep another package database. See
[Content capsule distribution](CONTENT_CAPSULE_DISTRIBUTION.md).

Home summary embeds those two projections and derives every capsule launcher
target from the catalog. Content with a bound viewer is launchable through that
viewer. People is a normal catalog-backed app capsule at `/apps/people/`; shells
must not synthesize it or replace its app-scoped launch authority with Home's.
Consumers may choose different presentation, but must preserve catalog roles,
viewer/content links, requirements, provider namespaces, and interface binding
availability without name-based inference.

The System-token-only contract audit keeps repository evidence separate under
its `development` field. That diagnostic inventory classifies source-only,
active-but-uninstalled, invalid-installed, installed-inactive, and
installed-active entries without projecting non-product entries into ordinary
catalog facts.

## Contract Audit

The contract audit is derived at request time. It does not maintain a separate
capsule or provider registry. Active first-party names come from
`components.json` plus checked-in manifests; installed state comes from the
runtime capsule tree; launch state comes from Home launch targets; provider and
Carrier state comes from the live `ProviderRegistry`; and generic invocation
bindings come from the same resolver used by
`/api/capsules/interfaces/invoke`.

The audit fails closed when an active first-party capsule is not installed, an
installed capsule is inactive, source and installed manifests disagree, a
manifest is invalid, a capsule requirement or viewer is unresolved, an external
requirement has no artifact at its canonical platform install path, a provider
namespace has no live registration, provider authority lacks a canonical
Runtime action, or a method is presented as executable without a generic Runtime
dispatch binding. User/high-risk, provider-path-only, and unbound descriptors
remain visible; they are not reported as generically executable.

## First-Party Capsule Descriptors

The first manifest-declared affordance descriptors now cover all first-party
app, viewer, shell, connector, content, and provider surfaces. The Home-facing
set includes `home`, `home-gui`, `home-cli`, `browser`, `wallet`, `wallet-metamask`,
`wallet-unisat`, `wallet-walletconnect`, `inbox`, `services`, `system`,
`library`, `documents`, `archive-manager`, `chat-room`, `chat`,
`agent`, `marketplace`, `gba-emulator`, and `gba-ucity`.

These descriptors give shells a shared way to answer "what can this capsule ask
for, and what risk/approval/audit shape does that imply?" The descriptors are
still not grants. Runtime route policy, launch tokens, provider authority,
Inbox/Wallet approval, and audit writes remain authoritative.

Provider descriptors are projected from existing provider authority metadata.
They do not add provider authority or make providers callable by shells. They
make the service-plane contract inspectable: Browser/Net/Exit infrastructure
methods are runtime-policy gated, while direct signing, payment, key release,
secret export, destructive storage, and protected-content render effects keep
explicit user approval metadata.

The Browser UI capsule is a Runtime projection with web assets. It declares
Browser-scoped page, display, exit selection, profile reset, and wallet-bridge
affordances so shells can inspect the UX contract, but it does not receive raw
Browser Engine, Exit, Net, Wallet, media relay, profile-storage, or cleanup
authority. Those effects stay behind Runtime-owned provider routes and gates.

## Completion Rule

A capsule interface is complete enough for both GUI and CLI when Runtime can
project:

- web launch target and attach policy
- CLI-readable identity, summary facts, and projection status
- declared affordances and method metadata
- required/provided capabilities
- gate/risk/approval metadata
- storage and namespace boundaries
- Carrier/service endpoints where relevant
- audit, provenance, signature, CID, payment, and DRM states

If a surface cannot be derived from Runtime facts, the shell must show it as
absent or incomplete rather than inventing it locally.
