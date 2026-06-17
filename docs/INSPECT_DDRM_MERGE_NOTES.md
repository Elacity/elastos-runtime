# Inspect ↔ DDRM integration notes

How `feat/capsule-inspector` connects with `feat/ddrm-hardening-and-creator-parity`.
Both branch from `main` (= `0.4.0`). Cross-branch analysis, non-destructive.

## Verdict — GO-WITH-NOTES (validated by merge dry-run)

**No model divergence.** DDRM adds no competing inspect/introspection surface and
does not restructure the capability or `ProviderRegistry` model the Inspector
depends on. The Inspector is cleanly the **governance/visibility layer over
DDRM's providers** — reflecting their authority/affordances and previewing the
gate a call would require, adding no provider authority of its own. This is the
agent-safe-computing commercial wedge, over real DDRM powers (key release,
decrypt render, rights decisions, chain broadcast).

**Merge order:** land DDRM first (hardening base + provider fleet), then
rebase/merge `feat/capsule-inspector` on top.

### Dry-run result (isolated worktree, nothing pushed)

A real `git merge --no-commit` of our branch onto the DDRM tip was performed in
an isolated worktree, reconciled, built, and tested. Outcome:

- **The merge auto-merges with ZERO conflict markers** — *every* file, including
  the HIGH `carrier_bridge.rs`. Git will report success. **The reconciliation
  below is therefore mandatory *and invisible to git* — it must be applied by
  hand even though nothing conflicts.**
- After applying the recipe: `cargo build -p elastos-server` succeeds; **20
  inspect tests + the carrier e2e test + runtime inspect/invoke tests all pass.**
- The silent carrier-gate break was **empirically confirmed**: removing the
  `required_action_for` inspect arm makes the carrier inspect test fail
  `capability_denied`; restoring it passes.

### Mandatory reconciliation recipe (apply in order at the real merge)

1. **(critical, silent — now ENFORCED by a tripwire)** In `provider_resource.rs`,
   add to `required_action_for` *before* the `_ => Action::Admin` default:
   ```rust
   "capsules" | "capsule" | "plan" | "self" => Action::Read,
   "revoke" => Action::Write,
   ```
   Without this, DDRM's op→action gate fail-closes the carrier inspect leg to
   `Admin`. Git does **not** flag it — but our branch now does. The canonical
   mapping lives in one place, `provider_resource::inspect_op_required_action`,
   and the test `carrier_inspect_ops_match_canonical_action_contract`
   (carrier_bridge.rs) drives a real carrier call per inspect op with a token
   minted at that action. The moment `required_action_for` disagrees, that test
   goes red at merge instead of breaking silently at runtime. Wire DDRM's inspect
   arm to delegate to `inspect_op_required_action` so the two cannot drift.
2. **(compile-break, NOT previously documented)** DDRM added a new field
   `audit_log: Arc<OnceLock<Arc<AuditLog>>>` to the shared `GatewayState`
   (`api/gateway.rs`). Our `api/gateway_tests/inspect.rs` constructs
   `GatewayState { … }` literally and must add
   `audit_log: Arc::new(std::sync::OnceLock::new()),` (the idiom DDRM uses in its
   other gateway test constructors). Otherwise the lib-test target won't compile.
3. **No manual action needed** for `registry.rs`, the `carrier_bridge.rs` validate
   block, and all `use`-line merges — git auto-merges these correctly, keeping
   both sides' additions (the earlier note implying manual import reconciliation
   was over-cautious).

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
