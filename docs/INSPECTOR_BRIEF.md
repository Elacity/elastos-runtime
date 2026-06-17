# Capsule Inspector & Metadata-Driven Reflection — Brief for Rong

*One-page status + direction. Reflects what is built and tested today.*

## Thesis

Take Self's one durable idea — a **live system you can open, inspect, and act
on** — and ground it in the things Self lacked and Elastos pioneered:
**capability security, signed provenance, and metadata-driven reflection (CAR)**.

> **Self's UX × CAR's metadata reflection × ElastOS capability/Carrier security.**

This directly addresses the three reasons Self stalled at Sun/IBM:
**(1) granularity** — the unit is the **capsule**, not the object;
**(2) OS perspective** — reflection is an OS-level, capability-gated runtime
surface, not an in-process toy; **(3) metadata-driven reflection** — the
capsule manifest's **typed interface** is the machine-readable contract.

## Built and tested today (on the product path)

- **Capsule Inspector** — read-only, object-centered view of every capsule:
  identity, manifest, **typed affordances** (risk/approval/audit +
  input/output schema), required capabilities, storage, Carrier, provenance,
  **live audit**, running status.
- **One canonical path** — served as an `inspect` provider on the shared
  `ProviderRegistry`, reached by *both* the browser gateway and the capsule
  Carrier bridge (no parallel trust system).
- **Security is the product** — capability-gated, fail-closed, scope-tiered;
  the surface never leaks a bearer token or raw signature (enforced by test).
- **Rich data sources** — installed-capsule manifests + runtime instances +
  registered (incl. sub-provider) schemes + the signed runtime audit log.
- **Quality bar** — pure decision cores (`inspect`, `invoke`) with exhaustive
  unit tests; full crates compile clean; ~290 tests green; nothing fabricated
  (honest gaps are documented, not hidden).

## Prototype in hand (today's increment)

A **metadata-driven invocation planner** (`elastos-runtime::invoke`): given an
affordance's typed metadata, it validates the call arguments against the
`input_schema` and derives the policy gate (capability action + approval +
audit) — *the metadata drives the call*. Pure and transport-agnostic; the
reflective kernel a real invoker would call.

## Direction (for your input — not assumed)

The kernel above is the bridge from **inspect → invoke → CAR**:

1. **Metadata-driven typed invoke** — runtime uses the affordance schema to
   validate/gate/marshal a capability-checked call. *(planner built)*
2. **Location-agnostic** — the same typed call routed locally or to a remote
   peer over Carrier, unchanged (Principle #4 = CAR's location transparency).
3. **Cross-language interop** — one metadata contract spanning WASM, microVM,
   and native capsules (CAR's JS/Java/Python ↔ C/C++ goal).

Steps 2–3 are foundational enough to be **architecture decisions for you**, not
a fait accompli. The brief is to walk in aligned, with the kernel demonstrable.

## Honest gaps

- `granted_capabilities` is empty — bearer-token caps have no central registry;
  surfacing observed grants needs a capability-event source recording
  resource+action.
- Provider-scheme detail is thin where no on-disk manifest exists.
- Invocation dispatch/marshalling/Carrier transport are intentionally unbuilt.

## Why now (time & commercialization)

We are **not** re-implementing Self; we have extracted its value and added the
security/decentralization layer the 1990s projects never had — that layer is
the moat and the commercial wedge: **agent-safe computing** (let humans and AI
agents act through capability-bounded, audited, inspectable, revocable
capsules). That story is what attracts partner investment, which is what buys
the time to do the deeper CAR work properly.
