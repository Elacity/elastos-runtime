# Shell ESP Boundary Map

This map is the durable public boundary for Home shells, ESP, Inspector, and
shared projection code. It is not a branch extraction log. Current truth belongs
in `state.md`, open work belongs in `TASKS.md`, and release history belongs in
`elastos/CHANGELOG.md`.

## Ground Rule

Extract protocol, facts, projections, and tests first. Extract visual shell UI
only after the facts it paints are real.

ESP is a logical protocol over Runtime facts. It must stay runtime-owned and
provider-aligned instead of becoming a browser-only API or a second authority
layer.

## Architecture Placement

ESP is aligned with ElastOS only when it is treated as a logical protocol over
runtime-owned facts, not as a new authority layer and not as a browser-only API.

| Piece | Belongs | Why |
| --- | --- | --- |
| ESP descriptor and route registry | Trusted runtime edge: `elastos/crates/elastos-server/src/api/` | Shells need a runtime-owned handshake. The route may be HTTP today, but the product contract is the fact/verb schema, not HTTP. |
| Capability checks, consent gates, request binding, token mint/spend, audit writes | Trusted core/runtime crates plus gateway enforcement | These are authority. They must not live in shell capsules, UI packages, or optional providers. |
| Capsule catalog, interface registry, launch/session state | Runtime/server API, backed by manifests and runtime state | These are authoritative projections of installed and running capsules. Shells render them; they do not invent them. |
| Inspector mirror over runtime state | Runtime-owned System provider surface | It needs privileged runtime visibility. It should expose a provider-shaped `elastos://inspect/*` contract without becoming an ordinary app capsule with broad hidden powers. |
| Provider-specific operations such as wallet, content, DID, decrypt, exit, browser engine | Provider capsules under `capsules/*-provider` | Providers own service semantics after a scheme. Runtime routes and audits; providers perform narrow effects. |
| ESP TypeScript fact/projection package | Outside the trusted core, under `elastos/esp/` | This is shared client/projection code for shells and tests. It must be pure, read-only, and carry no authority. |
| Home shell / second shell / visual shell UX | Shell-role capsules, currently `capsules/home-gui` and `capsules/home-cli` | Shells are replaceable projection and consent surfaces. They may ask; runtime decides. |
| Home shell host contract | Runtime-owned front door | `docs/HOME_SHELL_HOST_CONTRACT.md` defines the front-door contract for unlock, active-shell selection, one root-shell mount, child intents, and recovery. |
| System Inspector UX | App capsule, currently `capsules/system` | System is an admin/app surface over runtime facts, not part of the TCB. |
| Standalone Capsule Inspector | Optional app capsule | A standalone capsule-inspector is optional until System Inspector and shared `elastos/esp` projections are coherent. It must not vendor `spend_audit.js` or duplicate custody/audit projection logic. |
| Shell picker active-shell setting | Runtime-owned setting plus shell/app UI | Candidate shells come from catalog facts. Selection must not be stored only in UI state because it affects shell-role launch authority. |
| Visual components and render tests | Outside runtime, bundled into shell/app capsules | Pixels are not authority. UI components must consume facts and emit intents only. |
| Dev-only conformance harnesses | Outside runtime, under `elastos/esp` and `scripts/` | These keep code/docs/tests aligned without increasing the TCB. |

First-principles placement rule:

- If it proves, gates, mints, spends, routes, audits, or binds a principal, it
  belongs in runtime trusted code.
- If it implements a service behind `elastos://...` or `localhost://...`, it
  belongs in a provider capsule unless it needs privileged runtime
  introspection.
- If it renders, organizes, asks, previews, or explains, it belongs in a shell
  or app capsule.
- If it is only shared client typing or pure projection, it belongs outside the
  trusted runtime and is bundled into capsules as needed.

The current HTTP routes are edge adapters for browser-hosted shells. They must
not become the product identity. The durable contract remains:

`shell capsule -> runtime capability/session -> ESP facts/intents -> runtime gate -> provider/Carrier plane`

That keeps capsule-to-capsule and off-box behavior Carrier/provider aligned
while still letting `localhost` serve the local browser UI during development.

## Current Extracted Surface

| Piece | Current target | Boundary |
| --- | --- | --- |
| ESP initialize descriptor | `elastos/crates/elastos-server/src/api/gateway_esp.rs` | Descriptor only; no authority grant. |
| ESP contract docs | `docs/ESP_V0.md` | Current served schema, fact, verb, and invariant contract. |
| ESP tests | `elastos/crates/elastos-server/src/api/gateway_tests/esp.rs` | Keeps initialize negotiation and no-authority claims pinned. |
| ESP type package | `elastos/esp/` | Private non-TCB TypeScript shapes and pure projections. |
| System Inspector | `capsules/system` plus runtime inspect routes | Reference mirror/gate-preview UI over Runtime facts. |
| Home CLI terminal shell proof | `capsules/home-cli/browser` plus Runtime PTY routes | Runtime-owned PTY terminal shell. The browser `home-cli` terminal shell proof uses explicit start/events/input/resize/close routes; the capsule renders bytes and sends terminal input without host process authority. |
| Home GUI shell | `capsules/home-gui` | Graphical root shell over the same Runtime Home facts and shell host contract. |

## UI Framework Rule

Reusable visual shell components are optional and outside ESP. For a future
visual shell, first attempt plain ES modules or native Web Components inside a
shell/app capsule. Svelte is optional only as a capsule-local compiler, never as
an ESP protocol dependency or trusted runtime surface. Extract it only after
the System Inspector and shared `elastos/esp` projections are coherent.

## Current Claim Boundary

- The browser `home-cli` terminal shell is implemented and machine-tested.
  Exact-commit operator evidence is still required; no arbitrary third-party
  shell or shell-marketplace UX is claimed.
- Shell marketplace is not implemented.
- SSE projection stream as a required ESP transport is not product-ready.
- Standing grants are not implemented or exposed by ESP v0.
- Reach enforcement and reach halos are not implemented by ESP v0.
- High-risk affordance invocation remains Inbox/passkey gated or fails closed.

## Verification

Focused checks:

```bash
(cd elastos && cargo test -p elastos-server esp_initialize -- --nocapture)
(cd elastos && cargo test -p elastos-server inspect_action -- --nocapture)
(cd elastos && cargo test -p elastos-runtime inspect -- --nocapture)
```

Required handoff gate:

```bash
git diff --check
node scripts/home-entropy-check.mjs
(cd elastos && cargo fmt --all -- --check)
```
