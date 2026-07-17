# ESP Type Package

`@elastos/esp` is a private, non-TCB TypeScript package for ESP v0 shapes.

It mirrors the current Runtime descriptor served by
`GET /api/esp/initialize`. It does not sign, store tokens, call providers,
dispatch routes, open sockets, or make authority decisions.

Included in this slice:

- ESP initialize descriptor request and response shapes.
- Current catalog and interface fact shapes.
- Current Inspector fact and flow shapes.
- Current verb request/response shapes for Inspector approval and low-risk
  capsule interface invocation.
- Pure projection helpers for trust, custody, shell selection, consent
  validation, capsule detail, Home fleet, audit views, and authority separation.

Excluded from this slice:

- Reach halos.
- Reach enforcement.
- SSE ESP projection streams.
- Standing grants.
- Shell marketplace.
- Full second-shell product UX.
- Capability token request/consume flows.
- Affordance receipts.
- Svelte or framework UI.

Visual UI remains outside this package. If a shell needs reusable visual
components, first attempt plain ES modules or native Web Components bundled in a
shell/app capsule. Svelte may be used later only as an optional capsule-local UI compiler.
That may happen only after the headless projections and tests pass; compiled
components must paint ESP facts and emit intents only.

Facts include index signatures because shells must ignore unknown fact fields.
Verb request bodies intentionally do not; the Rust handlers use strict request
parsing where authority is involved.

Projection helpers are also non-TCB. They accept Runtime facts and return
render-ready summaries. They do not open transports, store local state, sign,
verify signatures, hold keys, hold tokens, dispatch providers, or invoke
Runtime operations.

The authority projection keeps trust material, verification, declared
permissions, executable bindings, and policy metadata separate. It never emits
an authorization verdict: missing evidence remains unknown, declared risk stays
advisory, and route/frame/HTTP presentation signals never grant authority.

Check:

```bash
node --experimental-strip-types --check esp_v0.ts
node --experimental-strip-types --check index.ts
node --experimental-strip-types check-esp-v0.mjs
node --experimental-strip-types --test projections.test.mjs
```
