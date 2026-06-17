# Capsule Inspector

> An object-centered, read-only introspection surface for ElastOS.
> Self's *mirrors* and *Morphic*, wrapped in our zero-ambient-authority model.

## Why

A computer should let you open up the living thing you're using and understand
exactly what it *is*, what it *can do*, what it's *allowed* to do, and where it
*came from* — without fear, and without having to trust a claim you can't see.

ElastOS already enforces a strong security model (no ambient authority,
capability-scoped provider calls, signed audit). But today that security is
**invisible**: a user installs a capsule and has no surface to see that it
*cannot* touch their keys, or what it has actually been doing. The Inspector
makes the existing guarantees **visible and demonstrable**.

This is also the human-facing answer to a gap `state.md` already records:

> System ... does not yet expose the fuller `elastos://` provider/object/
> capability discovery contract.

## How (the borrowed idea, re-secured)

Self is a live-object world: every running thing is an object you can inspect
and poke through *mirrors*, arranged in a direct-manipulation desktop
(*Morphic*). Self trusts everyone in one process. **We trust no one and prove
it.** So we keep Self's object-world UX and re-implement reflection as a
**permissioned mirror**:

| Self idea            | ElastOS realization                                  |
| -------------------- | ---------------------------------------------------- |
| Live object          | Capsule (signed, stateful, running)                  |
| Slots / messages     | Affordances (`interfaces[].methods` in capsule.json) |
| Mirror (reflection)  | Capability-gated, audited read-only inspect view     |
| Morphic world        | The Inspector UI (PC2 surface)                        |
| Transporter (export) | Signed capsule packaging (already exists)            |

Everything the Inspector shows is data the **trusted core already owns**
(manifests, capability grants, audit log, running instances, provenance). The
Inspector is therefore a *read-only projection* — not a new architecture.

### Lineage note: granularity, OS perspective, metadata-driven reflection

Self's two limits (per Rong Chen, and why Sun/IBM passed on it) are corrected
here by design: the unit of reflection is the **capsule**, not the object
(right granularity); reflection is an **OS-level** capability-gated surface, not
an in-process toy; and the surface is **metadata-driven** — the manifest's
typed interface (affordance `risk`/`approval`/`audit` + `input_schema`/
`output_schema`) is the machine-readable contract. The inspector projects that
metadata; the same metadata is the basis for typed, **location-agnostic**,
capability-gated invocation over Carrier — the modern realization of Elastos's
Component Assembly Runtime (CAR) idea. Today we surface the typed contract;
metadata-*driven invocation/marshalling* (and cross-language interop) is the
deeper CAR-scale direction, to be planned, not assumed.

## What

A read-only view, one screen per capsule, of nine fields:

1. **identity / DID** — capsule id, device/account DID where relevant
2. **manifest** — schema, role, type, entrypoint, version
3. **affordances** — declared interface methods + risk / approval / audit class
4. **required capabilities** — what the manifest asks for
5. **granted capabilities** — what was actually granted (and what was *denied*)
6. **storage namespaces** — `localhost://WebSpaces/...` scopes
7. **carrier endpoints** — gossip/peer endpoints, peer count
8. **provenance** — signature, signer DID, version, install time, CID
9. **logs / audit + running processes** — recent audited calls, live instances

## Security model (non-negotiable)

### Threat

A *system-wide* inspect view aggregates every capsule's manifest, capability
grants, and audit trail. In the runtime today this god-view is **shell-only**
(`request_handler.rs` — `ListCapsules`, `GrantCapability`, `RevokeCapability`,
`Launch`/`Stop` all reject non-shell callers). If the inspect surface returned
that view to any holder of a flat `inspect` capability, **any installed app
could enumerate every other capsule's powers and history** — an information-
disclosure / privilege-escalation hole, and the exact opposite of what this
feature exists to demonstrate. Visibility must therefore be *scoped*.

### Two tiers

| Capability grant        | Scope        | Reaches                                   | Sees                        |
| ----------------------- | ------------ | ----------------------------------------- | --------------------------- |
| `elastos://inspect/*`   | **System**   | `inspect/capsules`, `inspect/capsule`, `inspect/self` | every capsule     |
| `elastos://inspect/self`| **SelfOnly** | `inspect/self` only                       | only its own capsule record |

The tier is enforced first by the **capability layer itself**: validation
matches the requested URI against the token's resource *pattern*, so a
`elastos://inspect/self` grant (no wildcard) can never satisfy a request to
`elastos://inspect/capsules`. `crate::inspect::authorize_view` is the
defense-in-depth gate on top. Shell callers are always **System**, matching
existing orchestrator privilege. A caller holding neither grant (and not the
shell) is denied — the gate **fails closed**.

### Read vs. write (Phase 2)

Read endpoints require a `Read` inspect capability. The one mutating endpoint,
`elastos://inspect/revoke`, requires a **`Write`** inspect capability at
**System** scope. The two are separated by the *action* dimension of the
capability, not just the resource: a read-only inspect grant (`Read`) can never
satisfy a write endpoint, so the Inspector's normal read surface can never
drive a mutation (Principles #3, #16). Revocation only ever *reduces* authority
(fail-safe direction) and is audited (`inspect.revoke`).

This decision is implemented as a pure, unit-tested unit in the trusted core:
`elastos-runtime::inspect` (`authorize_view`, `InspectScope`). The runtime-side
handler MUST call `authorize_view` before returning any per-capsule detail.

### Invariants

- **Read-only.** The inspect surface exposes *no* write/sign/launch effect.
- **No new trust.** The Inspector capsule holds zero ambient authority and
  itself appears in the Inspector, subject to the same rules it reveals.
- **Permissioned mirror.** Every inspect call is capability-gated and audited
  like any other provider call.
- **Scope-bound, fail-closed.** A caller sees only what its scope allows;
  out-of-scope inspection is denied *and* audited. No capability ⇒ no data.
- **Least privilege.** The full-view product surface (this Inspector) requests
  `elastos://inspect/*`, which only the System surface can grant — it is a
  System-trusted surface, not a freely distributable app.

## Phasing

| Phase | What                                                         | Core impact            |
| ----- | ------------------------------------------------------------ | ---------------------- |
| **1** | Read-only Inspector UI + `elastos://inspect/*` contract      | None (additive)        |
| 2     | Invoke an affordance / revoke a grant from the UI            | Light (wire existing)  |
| 3     | Morphic-style direct manipulation: drag, re-wire providers   | Composition layer      |
| 4     | Transparent stored-vs-computed affordance backing (the Self  | Deeper ABI work        |
|       | "you can't tell if it's stored or computed" dream)           |                        |

Phases 1–2 deliver ~90% of the "see my system as living objects" experience for
~10% of the cost, with no rewrite. Phases 3–4 are where rewrite risk lives —
deliberately deferred.

## Phase 1 in this branch

This branch contains the Phase-1 starter:

- `elastos-runtime::inspect` — the scope/authorization core (`authorize_view`,
  `InspectScope`, `scope_for_grant`) with unit tests. Pure logic, provable in
  isolation.
- `RequestHandler::handle_inspect` — the runtime-side read-only handler backing
  `elastos://inspect/*`. It projects `CapsuleManager` + `AuditLog::recent_events`
  into the contract below, gated per request by `inspect::InspectScope`, and
  audits out-of-scope denials. Served on the existing `ResourceRequest` path —
  no new protocol variant. Conformance test:
  `tests/inspect_conformance.rs` proves a self-only caller cannot read another
  capsule.
- `capsules/capsule-inspector/` — a WASM capsule (UI) rendering the nine-field
  view. As the full-view product surface it requests `elastos://inspect/*`
  (System-granted). Ships with sample data so the UI renders standalone; uses
  the live handler when present.
- This doc — the architecture, security model, contract, and phasing.

## Transports & Carrier alignment

Per Principle #4 and `CARRIER.md`, the **capsule-facing contract is
Carrier-shaped**: a target (`elastos://inspect`), an operation, a payload, a
capability, and audit. The transport *underneath* is an adapter **below** the
capsule contract — `CARRIER.md` "Where HTTP Fits" explicitly classifies the
node-local HTTP control API as control-plane plumbing, *not* the Carrier
substrate, and Principle #4 lists "local loopback, HTTP, WebSocket,
postMessage, stdio, or in-process calls" as host adapters below the contract.
So using HTTP as the *transport* is aligned; what would violate alignment is a
capsule that *knows host routes*. The Inspector UI never does: all calls go
through one Carrier-shaped `inspectInvoke(operation, payload)`; swapping the
transport requires no UI change.

There are two transports to the **one** authority decision
(`crate::inspect::InspectScope`), satisfying Principle #7 (every path enforces
the same authority boundary):

| Caller | Transport (adapter) | Front door | Identity / scope |
| --- | --- | --- | --- |
| WASM / microVM capsule, agent | serial Carrier bridge → `carrier_invoke` | `RequestHandler::handle_inspect` | capability token → scope |
| Browser-hosted UI (this capsule) | node-local control API (`POST /api/provider/inspect/<op>` + `x-elastos-home-token`) | gateway → provider registry | signed home launch token → app → scope |

**Status (Principle #12 honesty).** The capsule/`carrier_invoke` path is
implemented and tested (`RequestHandler::handle_inspect`). The browser path is
**not yet wired, and is not a simple "add a provider" away** — a deeper
architecture finding blocks the naive convergence:

There is **no single capsule world**. Three separate domains exist, and the
gateway can reach none of the rich one:

| Domain | Holds | Reached by | Capsule detail |
| --- | --- | --- | --- |
| `elastos-runtime::CapsuleManager` | full manifest, capabilities, trust, audit | `RequestHandler::handle_inspect` | rich (the 9 fields) |
| `elastos-server::Runtime` (`RunningCapsuleInfo`) | id, name, status, type | the supervisor | thin |
| **Gateway** (browser front door) | **only `Arc<ProviderRegistry>`** | the browser UI | none (provider schemes) |

Evidence: `elastos-server` has zero references to `CapsuleManager`; the gateway
is started with only the registry (`supervisor.rs` → `start_gateway_server(addr,
Some(registry), …)`); `GatewayState` holds only `provider_registry`.

Consequences for the plan:

- A registry provider registered **in the supervisor cannot hold a
  `CapsuleManager`/`AuditLog`** — they don't exist in that process.
- The `handle_inspect` intercept **must not be retired**: it is the only path
  that reaches the rich `CapsuleManager` inspect (serving capsules/agents via
  the IO/carrier bridge).
- "One canonical path" (#10) is a real cross-process bridging effort, not a
  provider registration.

### Product-side provider (built)

The product home of inspect is now `elastos-server::inspect_provider` — an
`inspect` scheme provider on the **shared `ProviderRegistry`** that both product
transports dispatch through (`carrier_bridge` for capsules, the gateway for the
browser), so one provider serves both — the real one-canonical-path
convergence. It reuses `elastos_runtime::inspect::InspectScope` for the scope
label and enforces the #16 no-leak guarantee (proven by test:
`capsule_detail_renders_contract_without_leaking_authority`). Capsule data is
read through an `InspectSource` trait (implemented for `runtime::Runtime`) so
the provider is decoupled from where the server tracks capsules.

Ops implemented: `capsules` (System list), `capsule` (System detail, rich
nine-field projection from the retained manifest). Deferred: `self` (needs
caller-identity injection) and `revoke` (needs the gateway capability plane).

**Data sources (unified via `InspectSource`).** The provider reads through the
`InspectSource` trait; sources are composable:

- `RuntimeInspectSource` — the server `Runtime`'s running-capsule registry
  (capsules launched with a retained manifest → rich detail). Populated on the
  single-VM serve path.
- `CatalogInspectSource` — the installed-capsule catalog on disk
  (`<data_dir>/capsules/<name>/capsule.json`). Reads each capsule's **full
  manifest** (rich nine-field detail), marks it `running` when the scheme it
  `provides` is registered live (incl. sub-provider schemes), else `installed`,
  and attaches the **content CID** from `<data_dir>/components.json` as the
  provenance anchor (Principle #15). (`id = capsule:<name>`, path-traversal ids
  rejected.)
- `RegistryInspectSource` — the registered provider schemes from
  `ProviderRegistry`, including **sub-provider schemes** (`did`, `key`, `peer`,
  …) via `ProviderRegistry::sub_provider_schemes()`. Thin
  (`id = provider:<scheme>`); a source for built-in schemes with no on-disk
  capsule; not in the default aggregate.
- `AggregateInspectSource` — unions sources and de-dups by id.

**Live audit.** The provider takes an optional `AuditSource`. `AuthAuditSource`
reads the signed runtime audit log (`RuntimeAuditEventV1` in the auth state),
correlates events by `capsule_id` (the capsule name), and fills the detail
view's `audit` section — recent events (newest-first, capped) plus `total` and
`denied` counts. Wired on both serve paths. Reads run on a blocking task so the
async workers aren't stalled. Records are projected to safe fields only
(timestamp, event type, reason, success) — no signatures or handles (#16).

**Wired on both serve paths:**

- Single-VM serve (`elastos serve <microvm>`): `RuntimeInspectSource` → rich,
  populated end-to-end.
- Main product path: `Aggregate[RuntimeInspectSource, CatalogInspectSource]`
  on the shared registry the supervisor/gateway use — so the **browser
  Inspector lists installed capsules with their full manifests** and running
  status.

**Remaining enrichment (honest gap):** `granted_capabilities` is still empty on
the product path. ElastOS capabilities are bearer tokens with no central
per-capsule registry, and `RuntimeAuditEventV1` carries no resource/action — so
deriving the observed grant list needs a capability-event source that records
resource + action. Everything else is done: projection, scope, no-leak,
transport wiring, source aggregation, rich manifest detail, sub-provider
running-status coverage, and live audit (recent + counts).

The UI adapter targets the Carrier-shaped `inspect/<op>` contract and degrades
to sample data until the data source is populated on the browser path.

## Wire contract: `elastos://inspect/*` (read-only)

All operations are `read`. Responses are JSON. Every response is **filtered by
the caller's scope** (see Security model): `System` callers see all capsules;
`SelfOnly` callers see only their own record. A request for a capsule outside
the caller's scope returns `{ "error": "out_of_scope" }` and emits an audit
event — it never silently returns empty.

### `elastos://inspect/capsules` — list (System scope)

```json
{
  "scope": "system",
  "capsules": [
    { "id": "...", "name": "chat-room", "role": "shell", "type": "wasm", "state": "running" }
  ]
}
```

### `elastos://inspect/self` — detail of the calling capsule (any scope)

Returns the same detail shape as below, for `caller == target`. This is the
endpoint a `SelfOnly` capsule uses to introspect itself.

### `elastos://inspect/capsule` (params: `{ "id": "..." }`) — detail (System scope)

```json
{
  "id": "cap_chat_room_01",
  "name": "chat-room",
  "version": "0.1.0",
  "role": "shell",
  "type": "wasm",
  "description": "Peer-to-peer chat room",
  "author": "elastos",
  "identity": {
    "did": "did:key:z6Mk...",
    "cid": "bafy...",
    "trust_level": "verified",
    "signature_present": true,
    "signed_by": "gateway-did"
  },
  "manifest": { "schema": "elastos.capsule/v1", "entrypoint": "chat.wasm" },
  "affordances": [
    { "interface": "elastos.chat/v1", "id": "send", "risk": "write",
      "approval": "user", "audit": "event", "description": "Send a message",
      "input_schema": { "type": "object" }, "output_schema": { "type": "object" } },
    { "interface": "elastos.chat/v1", "id": "history", "risk": "read",
      "approval": "none", "audit": "summary", "description": "Read history",
      "input_schema": null, "output_schema": null }
  ],
  "required_capabilities": ["elastos://carrier/*", "elastos://storage/chat"],
  "granted_capabilities": [
    { "resource": "elastos://carrier/*", "action": "message", "granted": true,
      "token_id": "tok_...", "expiry": 1781990400 },
    { "resource": "elastos://did/*", "action": "read", "granted": false }
  ],
  "storage_namespaces": ["localhost://WebSpaces/chat-room/"],
  "carrier": { "enabled": true, "endpoints": ["gossip://..."], "peers": 1 },
  "provenance": { "signed_by": "gateway-did", "version": "0.1.0",
                  "installed_at": 1781817600, "cid": "bafy..." },
  "audit": {
    "counts": { "total_today": 14, "user_approved": 2, "denied": 1 },
    "recent": [
      { "ts": 1781990100, "event": "capability.use", "detail": "carrier/* message", "success": true },
      { "ts": 1781990050, "event": "capability.denied", "detail": "did/* read", "success": false }
    ]
  },
  "processes": [
    { "kind": "wasm", "instance": "#4", "memory_mb": 12, "uptime_s": 10800 }
  ]
}
```

The runtime-side handler maps these fields from existing sources:
`CapsuleManager::list()` / `get()` (id, manifest, state, cid, trust_level) and
`AuditLog::recent_events` (recent events + counts). No new state is introduced.

### `elastos://inspect/revoke` (params: `{ "token_id": "<32 hex>" }`) — write

The one mutating endpoint. Requires a **`Write`** inspect capability at
**System** scope (or the shell). Revokes the capability token by id via
`CapabilityManager::revoke`, reducing authority only. Returns `Ok` on success;
`permission_denied` for a read-only or self-only caller; `invalid_token_id` for
a malformed id. The action is audited (`inspect.revoke`) in addition to the
capability manager's own revocation audit.

### `elastos://inspect/plan` (params: `{ "id", "interface", "method", "args" }`) — read

Metadata-driven invocation **preview** (read-only dry-run; dispatches no
effect). Looks up the affordance's typed metadata, validates `args` against its
`input_schema`, and returns the gate the call *would* require:

```json
{ "valid": true, "capability_action": "write", "approval": "user", "audit": "event" }
```

or, when the args don't satisfy the contract:

```json
{ "valid": false, "error": "missing_required_field", "field": "body" }
```

This is the reflective half of the CAR invoke kernel (`elastos-runtime::invoke`).
Effect *dispatch* (and the location-agnostic Carrier / cross-language transport)
is intentionally not implemented — that architecture is to be planned.

### Why `granted_capabilities` is observed, not enumerated

ElastOS capabilities are **bearer-token object-capabilities**: a grant is an
unforgeable signed token held by the grantee, validated by signature +
revocation epoch + revoked-set. The runtime keeps **no central per-capsule
registry of active grants** (only a revoked-set and use-counts). So the
authoritative, safe-to-display record of authority is the **audit log** — what
was actually granted and used. `granted_capabilities` is therefore *observed
from audit*, by design, not read from a token table.

This is also why Principle #16 (UI Surfaces Must Not Be Authority) is load
bearing here: the inspector projects an allow-listed set of safe fields and
**never** echoes a bearer token, a raw signature, or any mutation handle. The
raw manifest `signature` is reduced to `signature_present: true`. This is
enforced by test (`inspect_detail_renders_contract_without_leaking_authority`).
