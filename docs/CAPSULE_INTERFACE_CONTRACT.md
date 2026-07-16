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
| Affordances | Manifest-declared `interfaces[*].methods` projected through `/api/capsules/interfaces` | Affordances describe possible calls. They are not grants. |
| Gate metadata | ESP verbs, method risk/gate descriptors, Runtime route policy, and Inspector gate preview | Route-specific Runtime gates decide authority. |
| Audit / mirror view | Catalog trust/provenance fields plus System Inspector mirrors | Ordinary shells get redacted facts. Privileged mirrors stay System/Runtime-owned. |

`/api/capsules/catalog` also derives a `projection` object for each capsule
using schema `elastos.capsule.projection/v1`. This is not another manifest
surface. It is Runtime's compact shell-facing status for web, CLI, facts,
affordances, gates, audit/mirror, and Carrier/service readiness, derived from
the manifest, launch targets, provider namespace, capabilities, and trust
projection that Runtime already knows.

## Shell Rules

- `home-gui` and `home-cli` consume the same Runtime facts.
- `home-gui` renders desktop, windows, launcher, taskbar, and app chrome as
  trusted host-loaded GUI shell code in the current 0.5.0 implementation.
- `home-cli` renders commands, context, capsule facts, affordances, gates, and
  approval hints. Runtime-policy invoke attempts go through
  `/api/capsules/interfaces/invoke`; user/high-risk methods stay blocked or move
  through the owning approval surface.
- Shells may ask Home to open a visible capsule with a typed host intent.
- Shells must not call providers directly for authority-bearing effects.
- Shells must not use ambient same-origin state to switch shells or dispatch
  provider operations.
- Browser iframes are presentation containers. Same-origin iframe transport is
  local API compatibility, not an authority grant or capsule isolation proof.
- Shells must ignore unknown fact fields.

## Current Intent Path

Browser-hosted shells currently use same-origin host messages such as
`home:open-target`, `home:close-self`, and `home:refresh-summary`.

Those messages are local adapter details. The durable model is:

`shell capsule -> Runtime capability/session -> ESP facts/intents -> Runtime gate -> provider/Carrier plane`

Future Carrier transport must preserve the same schemas, gates, consent path,
dispatch path, and audit semantics.

## Core 0.5.0 Capsule Descriptors

The first manifest-declared affordance descriptors now cover all first-party
app, viewer, shell, connector, content, and provider surfaces. The Home-facing
set includes `home`, `home-gui`, `home-cli`, `browser`, `wallet`, `wallet-metamask`,
`wallet-unisat`, `wallet-walletconnect`, `inbox`, `services`, `system`,
`library`, `documents`, `archive-manager`, `chat-room`, `chat`, `chat-wasm`,
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
