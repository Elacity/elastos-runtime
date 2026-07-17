# ESP v0 - ElastOS Shell Protocol

ESP is the small shell-facing contract over the Runtime facts exposed by the
0.6 development stack. A shell is a projection and consent surface. It does not mint
authority, hold keys, bypass capability checks, dispatch providers directly, or
invent provenance.

The shared capsule web/CLI/fact/affordance/gate/audit model is documented in
[`CAPSULE_INTERFACE_CONTRACT.md`](CAPSULE_INTERFACE_CONTRACT.md).

This document describes the ESP v0 slice implemented on the current 0.6
development branch.

The current terminal implementation is the `home-cli` shell. ESP is its
protocol contract, not a separate product shell.

## Non-Negotiables

- The trusted Runtime/provider core owns authority.
- Shells render facts and request consent; they do not perform effects.
- Route-specific gates remain the gates. ESP does not weaken them.
- Gate previews are preview-only and must not mutate or dispatch.
- Approved Inspector dispatch goes through Inbox, fresh passkey proof, and
  `ProviderRegistry`.
- Facts may gain fields. Shells must ignore unknown fact fields.
- Verb input bodies remain strict where the Rust handlers use
  `#[serde(deny_unknown_fields)]`.

## Trust And Authority Invariants

- Verification proves evidence only; it does not authorize or make a method executable.
- Declared risk is advisory metadata; Runtime bindings and route policy decide executability and authority.
- Missing trust, permission, binding, or policy evidence is unknown, never safe.
- Routes, frames, iframe placement, and HTTP success are transport or presentation facts, not authority.
- Effect completion requires an exact request binding and matching Runtime result receipt.

ESP projects these as separate axes. Trust material and verification evidence do
not become permissions. Manifest permissions do not become grants. A method is
generically executable only when Runtime projects a concrete executable binding,
and even that binding does not replace the route's launch-token and policy gate.
No shell may infer authority from successful loading, same-origin placement, or
an HTTP success response.

## Initialize

`GET /api/esp/initialize` returns the static ESP descriptor for
`protocol: "elastos-shell-protocol"` over `transport: "http-json"` with
`transport_scope: "local_runtime_adapter"`.

`POST /api/esp/initialize` accepts:

```json
{
  "esp_version": "0",
  "accepts": ["elastos.inspect.gate-preview/v1"]
}
```

If `esp_version` is omitted, the Runtime treats it as `"0"`. Any other version
fails with HTTP `400`, `code: "unsupported_esp_version"`, and
`supported: ["0"]`.

The response is `elastos.esp.initialize/v0` and includes:

- `supported_schemas`
- `facts`
- `verbs`
- `invariants`
- `accepted`
- `unsupported`

This endpoint is only a descriptor. It reads no registry state and grants no
authority.

The HTTP `method` and `route` fields describe the current local gateway adapter
for Home/System browser capsules. They are not the ESP authority model. The
transport-neutral surface is the schema, family/name, operation, auth,
authority, verb, and invariant metadata. A future Carrier adapter may expose the
same ESP schemas only by preserving the same Runtime gates, consent path,
ProviderRegistry dispatch path, and audit semantics.

The currently served `supported_schemas` list is:

- `elastos.capsules.catalog/v1`
- `elastos.capsules.interfaces/v1`
- `elastos.inspect.capsules/v1`
- `elastos.inspect.object/v1`
- `elastos.inspect.gate-preview/v1`
- `elastos.inspect.action-request/v1`
- `elastos.inspect.action-result/v1`
- `elastos.esp.request-binding/v1`
- `elastos.inspect.dispatch-result/v1`
- `elastos.capsules.invoke-result/v1`

## Projection Facts

| Family | Schema | Operation | Local route | Gate |
| --- | --- | --- | --- | --- |
| Capsule catalog | `elastos.capsules.catalog/v1` | `capsules.catalog` | `GET /api/capsules/catalog` | Home, System, Marketplace, or launchable shell token |
| Capsule interfaces | `elastos.capsules.interfaces/v1` | `capsules.interfaces` | `GET /api/capsules/interfaces` | Home, System, Marketplace, or launchable shell token |
| Inspector list | `elastos.inspect.capsules/v1` | `inspect.capsules` | `POST /api/provider/inspect/capsules` | System launch token |
| Inspector object | `elastos.inspect.object/v1` | `inspect.object` | `POST /api/provider/inspect/capsule` | System launch token |
| Gate preview | `elastos.inspect.gate-preview/v1` | `inspect.gate_preview` | `POST /api/provider/inspect/plan` | System launch token |
| Inspector action request | `elastos.inspect.action-request/v1` | `inspect.request_act` | `POST /api/provider/inspect/request_act` | System launch token |

The catalog and interface facts are read-only manifest/catalog projections.
They expose declared affordances, but declarations are not grants.

Inspector facts are the Self-style mirror surface, re-secured for ElastOS:
`elastos://inspect/*` is System scope and `elastos://inspect/self` is SelfOnly
scope in the pure Runtime gate. Product browser routing currently keeps
`/api/provider/inspect/self` System-only until a caller-bound ordinary-capsule
SelfOnly route is explicitly wired and tested.

`elastos.esp.request-binding/v1`, `elastos.inspect.action-result/v1`, and
`elastos.inspect.dispatch-result/v1` are flow schemas, not standalone projection
routes. The request binding covers the stable request ID, principal, capsule,
interface where applicable, method, Runtime resource, and canonical request
body. The dispatch result is produced only after Inbox approval,
same-principal passkey proof, plan revalidation, and internal
`dispatch_approved` through the Inspect provider. Neither schema grants
authority, and neither exposes `dispatch_approved` as a shell-callable route.

`elastos.capsules.invoke-result/v1` carries the same exact request binding for
generic Runtime invocations. Shells must reject a result whose request ID or any
bound field differs from the request. HTTP success, a route suitable for
navigation, iframe messages, and unrelated provider output are never completion
proof.

## Consent And Act Verbs

| Verb | Route | Gate | Effect |
| --- | --- | --- | --- |
| `inspect.request_act` | `POST /api/provider/inspect/request_act` | System launch token | Stores a pending Inspector action request bound to its request ID, principal, capsule, method, resources, and body. |
| `inbox.approve_inspect_action` | `POST /api/apps/inbox/actions` | Inbox launch token plus fresh same-principal passkey Home token | Revalidates the exact request binding and authority plan, dispatches through `ProviderRegistry`, and returns a matching Runtime receipt. |
| `inbox.deny_inspect_action` | `POST /api/apps/inbox/actions` | Inbox launch token | Marks only the exactly bound Inspector action request denied and returns its matching receipt without dispatch. |
| `capsule.invoke_runtime_policy_affordance` | `POST /api/capsules/interfaces/invoke` | Target capsule launch token | Invokes only executable generic Runtime bindings and returns the exact request ID, principal, capsule, interface, method, resource, and body binding. |

Generic invocation requires both `executable: true` and a concrete
`handler_kind: "runtime"` binding in the Runtime-derived interface projection.
Provider-path-only, unbound, unknown, and approval-required operations fail
closed. High-risk and user-approval affordance invocation still uses the live,
proven Inspector action path: preview, request, Inbox approval or denial,
revalidate, provider dispatch, audit.

The generic invoke request must carry a caller-generated stable `request_id`.
Runtime authenticates the principal, resolves the declared method and resource,
hashes the canonical input body, and returns those exact facts in
`request_binding`. Home CLI validates that binding before rendering a confirmed
result; a route in `output` means only that navigation is available.

## Current UX Surface

System contains the reference Inspector UI. It calls only:

- `capsules`
- `capsule`
- `plan`
- `request_act`

It does not call `dispatch_approved` or `revoke`. `dispatch_approved` is an
internal provider operation reached only after Inbox approval.

## Current Scope

In scope for ESP v0:

- ESP naming and shell-as-projection framing.
- Capsule Inspector as a live object mirror.
- System vs SelfOnly inspection scope.
- Metadata-driven gate preview.
- Inbox-gated approved dispatch.

Out of scope for this branch:

- dDRM, DKMS, content-market, creator, marketplace, standing grants, shell
  marketplace, Three.js/vendor assets, reach/egress enforcement, and any second
  framework shell.
- Svelte or other framework UI. If a future visual shell needs components, the
  first implementation path is plain ES modules or native Web Components inside
  a shell/app capsule. Svelte is allowed only as an optional capsule-local compiler. It is never an ESP protocol dependency or trusted runtime surface.
- Any claim that `validate-and-consume`, standing-grant dispatch, reach halos, or
  SSE ESP streams are product-ready unless the current branch implements and
  tests them.

Current claim boundary:

- Standing grants are not implemented or exposed by ESP v0.
- Reach enforcement and reach halos are not implemented by ESP v0.
- SSE ESP projection streams are not product-ready; ESP v0 is the current
  initialize descriptor plus tested HTTP route/fact projections.
- Shell marketplace is not implemented.
- Full second-shell product UX is not complete. The browser-facing `home-cli`
  terminal shell is implemented and machine-tested, but its current commit
  still needs operator-profile evidence; no additional framework shell is
  implemented.
- The browser-facing `home-cli` product surface is a Runtime-owned PTY
  terminal through explicit start/events/input/resize/close routes. Runtime owns
  the process, PTY, stream ticket, launch-token gate, dimensions, and lifecycle;
  `home-cli` renders PTY bytes with a capsule-local xterm.js terminal and sends
  raw terminal input. xterm renders PTY bytes without receiving host process
  authority. Product acceptance requires exact-commit operator evidence, and
  any later Home shell behavior change requires a new review before release.
- No browser-side command projection remains. The browser wrapper does not
  fetch ESP/catalog facts, render a command form, or interpret shell commands;
  the `home-cli` capsule process owns the TUI/line-mode command surface through
  the Runtime-owned PTY.

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
