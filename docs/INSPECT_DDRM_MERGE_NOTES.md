# Inspect ↔ DDRM integration notes

How `feat/capsule-inspector` connects with `feat/ddrm-hardening-and-creator-parity`.
Both branch from `main` (= `0.4.0`). Cross-branch analysis, non-destructive.

## Verdict

**No model divergence.** DDRM adds no competing inspect/introspection surface and
does not restructure the capability or `ProviderRegistry` model the Inspector
depends on. The Inspector is cleanly the **governance/visibility layer over
DDRM's providers** — it reflects their authority/affordances and previews the
gate a call would require, adding no provider authority of its own. This is the
agent-safe-computing commercial wedge, made concrete over real DDRM powers
(key release, decrypt render, rights decisions, chain broadcast).

**Merge order:** land DDRM first (hardening base + provider fleet), then
rebase/merge `feat/capsule-inspector` on top.

## Conflict matrix (3 LOW, 1 HIGH)

| File | Risk | Why |
| --- | --- | --- |
| `runtime.rs` | NONE | DDRM does not touch it; our `RunningCapsuleInfo.manifest` field is sole. |
| `gateway_provider_proxy.rs` | NONE | DDRM does not touch it; our `inspect` allow-list arm is sole. |
| `provider/registry.rs` | LOW | Additive & ~220 lines apart: DDRM appends `encrypt`/`publish`/`media` to `RESERVED_SUB_NAMES`; we add `sub_provider_schemes()`. (Bonus: those new sub-names become discoverable through our method for free.) |
| `provider_resource.rs` | LOW | DDRM adds a new fn `required_action_for(op)` above the match + an `Action` import; we add one `"inspect"` arm inside `build_capability_resource`. Different regions; only the `use` line needs a trivial textual merge. |
| `carrier_bridge.rs` | **HIGH** | See below — silent semantic break. |

## The HIGH conflict (must fix by hand at merge — it auto-merges *green* but breaks at runtime)

DDRM rewrote the carrier capability gate in `handle_request`: instead of
`validate(…, token.action(), …)` it computes
`required_action_for(&dispatch.operation)` and validates against *that*.
DDRM's `required_action_for` map does **not** list inspect ops, so they hit its
`_ => Action::Admin` fail-closed default.

Consequence after merge: a carrier `inspect` call gated at `Read` (our model,
our test) is required to be `Admin` → **`capability_denied`**. This breaks the
live carrier inspect leg *and* our test
`carrier_invoke_reaches_inspect_provider_with_capability`. The two edits sit
~430 lines apart, so **git auto-merges without a conflict marker** — the break
is silent.

**Required reconciliation (in `required_action_for`):**

```rust
"capsules" | "capsule" | "plan" | "self" => Action::Read,
"revoke" => Action::Write,
```

Plus reconcile the duplicate `use …provider_resource::{…}` import.

## Synergy: DDRM providers the Inspector now reflects

DDRM provider capsules express powers via `authority.capabilities[]`
(resource / actions / operations) + `audit_events` — **not** `interfaces[].methods`.
Our projection now surfaces `authority` (this commit), so these are visible:

- **key-provider** `elastos://key/*` — ops `status`, **`release`**; audits `key.release.denied`.
- **decrypt-provider** `elastos://decrypt/*` — ops `open_session`, **`render`**.
- **rights-provider** `elastos://rights/*` — `has_access_by_content_id`, `can_stream`, `can_download`.
- **encrypt-provider** `elastos://encrypt/*` — **`seal`**.
- **publish-provider** `elastos://publish/*` — **`prepare_publish`**.
- **chain-provider** `elastos://chain/*` — **`broadcast_transaction`**, `node_lifecycle`.

Auto-pickup confirmed: `CatalogInspectSource` reads each capsule's `capsule.json`
and `RegistryInspectSource` lists `schemes() ∪ sub_provider_schemes()`, so DDRM's
providers and sub-names appear with no inspect-side code change.

## Follow-ups this implies

- **At merge:** apply the `required_action_for` reconciliation above (HIGH).
- **Invoke-dispatch (the wedge's "act" step):** build it to consult DDRM's
  `required_action_for` as the authoritative op→action classifier, so the
  planner's gate matches what the carrier bridge enforces. Best done *after* the
  DDRM merge so that map is present.
- **`build_capsule_view` parity:** the embedded-runtime projection
  (`request_handler.rs`) should mirror the new `authority` field for #12 parity.
- **Coordination gap:** the v0.5 / "v2" line (Anders) is **not on the remote**,
  so it could not be diffed. Push it (or a snapshot) to enable the same
  connection check before it and these two branches converge.
