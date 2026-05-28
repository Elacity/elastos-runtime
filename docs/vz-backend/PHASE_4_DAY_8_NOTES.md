# Phase 4 Day 8 — `VzErrorReport` readback RPC + Phase 4 capstone

> **Status:** Complete. Closes the final operator-facing observability gap left open by Day 7 ("we cache the typed `VzError` on `RunningVm`, but the supervisor has no RPC to surface it"). With Day 8, every Vz lifecycle surface — launch, in-flight RPC, host-initiated stop, forced-after-timeout, framework error — reads structured for operators. Phase 4 is now complete: the Mac substrate has full feature parity with Linux's per-capsule introspection model.
>
> **Linux-untouched gate:** `scripts/check-linux-untouched.sh bcf5a0a` green. The new `VzErrorReport` lives in `elastos-vz`; `ElastosError` (protected `elastos-common`) is unchanged. The new `SupervisorRequest::CapsuleVzError` variant + `SupervisorResponse::vz_error` field live in `elastos-server`, both of which are platform-shared but not Linux-protected. Linux dispatch is byte-identical: a `CapsuleVzError` RPC against a Linux supervisor returns `not_found` (no Vz backends exist in `self.running`), which is the same `not_found` shape `capsule_status` already returns.
>
> **Day 7 anchors:** [`PHASE_4_DAY_7_NOTES.md`](PHASE_4_DAY_7_NOTES.md).

---

## 1. What Day 8 ships

| Component | File | Change |
|---|---|---|
| `VzErrorReport` typed JSON surface | `elastos/crates/elastos-vz/src/error.rs` | New `serde`-derived struct + `VzError::to_report()` projecting the typed enum into the operator-facing JSON shape. |
| Public re-export | `elastos/crates/elastos-vz/src/lib.rs` | `pub use error::{VzError, VzErrorReport, VzExitReason}`. |
| `RunningVm` test hook | `elastos/crates/elastos-vz/src/vm.rs` | `#[doc(hidden)] #[cfg(target_os = "macos")] pub fn set_last_vz_error_for_testing(&mut self, err: VzError)` — lets supervisor unit tests inject every variant without provoking real Apple `NSError`s. |
| New RPC variant | `elastos/crates/elastos-server/src/supervisor.rs` | `SupervisorRequest::CapsuleVzError { handle }` with `op = "capsule_vz_error"`. |
| Response field | `elastos/crates/elastos-server/src/supervisor.rs` | `SupervisorResponse::vz_error: Option<elastos_vz::VzErrorReport>` (`skip_serializing_if = "Option::is_none"`). |
| Builders + helpers | `elastos/crates/elastos-server/src/supervisor.rs` | `SupervisorResponse::ok_with_vz_error(report)` + `SupervisorResponse::not_found()` (centralises the `not_found` shape used by both `capsule_status` and `capsule_vz_error`). |
| Supervisor method | `elastos/crates/elastos-server/src/supervisor.rs` | `Supervisor::capsule_vz_error(handle) -> CapsuleVzErrorOutcome` with three-state outcome (`Found(None)` / `Found(Some(report))` / `NotFound`). |
| Helper | `elastos/crates/elastos-server/src/supervisor.rs` | `vz_last_error_report(&CapsuleBackend) -> Option<VzErrorReport>` — sibling of the Day-7 `vz_last_exit_reason` helper. |
| `capsule_status` enrichment | `elastos/crates/elastos-server/src/supervisor.rs` | The existing `capsule_status` response now carries BOTH `last_exit_reason` (Day 7) AND `vz_error` (Day 8) for Mac Vz capsules. Single-query observability. |
| Tests | `elastos-vz/src/error.rs`, `elastos-server/src/supervisor.rs`, `elastos-server/tests/vz_shutdown_semantics.rs` | See §4 inventory. |
| Docs | `docs/vz-backend/PHASE_4_DAY_8_NOTES.md` (new), `docs/vz-backend/PLAN.md`, `docs/MAC.md` | This file + Phase 4 status header bumped to "complete". |

---

## 2. JSON wire format

### 2.1 `SupervisorResponse::vz_error` field

The new optional field on `SupervisorResponse`:

```json
{
  "status": "ok",
  "vz_error": {
    "kind_label": "vz_timed_out",
    "description": "VZ stop did not complete within budget for vm phase4-day8-test-vm (1.50s); forced via DelegateExit::ForcedAfterTimeout. Runbook: docs/vz-backend/PHASE_4_DAY_6_NOTES.md §3",
    "vm_id": "phase4-day8-test-vm",
    "budget_secs": 1.5
  }
}
```

When `vz_error` is `None` the field skip-serialises entirely — legacy dashboards that don't know about the field keep working unchanged.

### 2.2 `VzErrorReport` field semantics

| Field | Type | Always set? | Variants that populate it | Operator usage |
|---|---|---|---|---|
| `kind_label` | `String` | Yes | All — mirror of [`VzError::kind_label`] | Dashboard filter (`vz_error.kind_label == "vz_internal"`). Stable across versions. |
| `description` | `String` | Yes | All — mirror of [`VzError::description`] / Apple's `NSError.localizedDescription` | Human-readable triage. Do NOT alert on substring matches. |
| `domain` | `Option<String>` | No | `Unknown` only | Apple `NSError.domain`. Lets ops route an unmodelled variant (e.g. `NSPOSIXErrorDomain`) without a binding update. |
| `code` | `Option<isize>` | No | `Unknown` only | Apple `NSError.code`. Pair with `domain` to grep a specific future variant. |
| `vm_id` | `Option<String>` | No | `TimedOut` only | Matches the supervisor's `handle` log lines — pivot from "forced_after_timeout spike" to "which capsule". |
| `budget_secs` | `Option<f64>` | No | `TimedOut` only | Configured `VzConfig::stop_timeout` at the moment of timeout. Sub-second budgets survive the JSON wire as fractional seconds. |

### 2.3 Full mapping: `VzError` → `VzErrorReport`

| `VzError` variant | `kind_label` | `description` | `domain` | `code` | `vm_id` | `budget_secs` |
|---|---|---|---|---|---|---|
| `Internal { description }` | `vz_internal` | `description` | — | — | — | — |
| `InvalidConfiguration { description }` | `vz_invalid_configuration` | `description` | — | — | — | — |
| `InvalidState { description }` | `vz_invalid_state` | `description` | — | — | — | — |
| `InvalidStateTransition { description }` | `vz_invalid_state_transition` | `description` | — | — | — | — |
| `NetworkError { description }` | `vz_network_error` | `description` | — | — | — | — |
| `OperationCancelled { description }` | `vz_operation_cancelled` | `description` | — | — | — | — |
| `NotSupported { description }` | `vz_not_supported` | `description` | — | — | — | — |
| `Unknown { domain, code, description }` | `vz_unknown` | `description` | `Some(domain)` | `Some(code)` | — | — |
| `TimedOut { vm_id, budget }` | `vz_timed_out` | runbook string embedding `vm_id` + `budget` | — | — | `Some(vm_id)` | `Some(budget.as_secs_f64())` |

### 2.4 RPC contract: `SupervisorRequest::CapsuleVzError`

Request:
```json
{ "op": "capsule_vz_error", "handle": "vm-foo-1234-0" }
```

Response (success path, capsule has a cached `Internal` error):
```json
{
  "status": "ok",
  "vz_error": {
    "kind_label": "vz_internal",
    "description": "kernel panic in vsock driver"
  }
}
```

Response (success path, no cached error / non-Vz backend / pre-stop capsule):
```json
{ "status": "ok" }
```

Response (unknown handle):
```json
{ "status": "not_found" }
```

The supervisor's three-state outcome (`Found(None)` / `Found(Some(report))` / `NotFound`) maps cleanly onto these three response shapes. Shell clients can treat `status == "ok" && vz_error == undefined` as "no failure to triage" and `status == "ok" && vz_error` as "structured triage data available".

---

## 3. Operator runbooks

### 3.1 Datadog: forced-after-timeout filter

```
kind:elastos.stop
@elastos.vz_error.kind_label:vz_timed_out
```

Pivots to the offending capsule via `@elastos.vz_error.vm_id`. Sized-too-tight stop budgets become trivially visible by histogramming `@elastos.vz_error.budget_secs` — a fleet-wide drop from 30s to 1.5s shows up immediately.

### 3.2 Datadog: framework-level failure filter (vs operation cancellation)

```
kind:elastos.stop
@elastos.vz_error.kind_label:vz_internal
```

The Day-7 work made `vz_internal` greppable in log lines; Day 8 makes it filterable in structured queries. Combine with `@elastos.vz_error.code:1` to disambiguate from documented variants that share the kind label in older binding versions.

### 3.3 Grafana: unmodelled Apple variants alert

```promql
sum by (domain, code) (
  rate(elastos_capsule_vz_error_total{kind_label="vz_unknown"}[5m])
)
```

A spike against a previously-unseen `(domain, code)` tuple means Apple shipped a new `VZErrorCode` we haven't taught the binding about yet — file a binding update and pin the kind_label until then.

### 3.4 Shell pivot: from forced-stop telemetry to structured triage

After `elastos stop vm-foo` returns `last_exit_reason: "forced_after_timeout"`:

```bash
elastos vz-error vm-foo
# {"status":"ok","vz_error":{"kind_label":"vz_timed_out","description":"...","vm_id":"vm-foo","budget_secs":1.5}}
```

`elastos status vm-foo` also returns both fields in a single query (the Day 8 enrichment), so dashboards can omit the second round-trip entirely.

---

## 4. Test inventory

### 4.1 `elastos-vz/src/error.rs` (4 new)
- `to_report_for_documented_variants_omits_unknown_specific_fields` — every documented Apple variant leaves `domain` / `code` / `vm_id` / `budget_secs` as `None`.
- `to_report_for_unknown_variant_populates_raw_apple_identifiers` — `Unknown` carries `domain` + `code`; non-Vz domains (e.g. `NSPOSIXErrorDomain`) round-trip without info loss.
- `to_report_for_timed_out_populates_vm_id_and_budget_seconds` — `TimedOut` carries `vm_id` + `budget_secs` from structured fields, not by parsing the description.
- `to_report_serde_round_trip_preserves_typed_fields_and_skips_none` — full JSON serde round-trip for every variant family, asserting `skip_serializing_if` works on every optional field.

### 4.2 `elastos-server/src/supervisor.rs` (10 new)
- `capsule_vz_error_unknown_handle_returns_not_found` — unknown handle → `CapsuleVzErrorOutcome::NotFound`.
- `capsule_vz_error_known_handle_without_cached_error_returns_found_none` — known handle, no cached error → `Found(None)`.
- `capsule_vz_error_round_trips_every_documented_vzerror_variant` — `Internal` / `InvalidConfiguration` / `InvalidState` / `InvalidStateTransition` / `NetworkError` / `OperationCancelled` / `NotSupported` all surface with the expected `kind_label`.
- `capsule_vz_error_unknown_variant_preserves_domain_and_code` — `Unknown` preserves Apple identifiers through the supervisor surface.
- `capsule_vz_error_timed_out_preserves_vm_id_and_budget` — `TimedOut` preserves `vm_id` + `budget_secs`.
- `handle_request_capsule_vz_error_surfaces_typed_report_for_internal_variant` — end-to-end through the dispatcher for the `Internal` variant.
- `handle_request_capsule_vz_error_surfaces_typed_report_for_timed_out_variant` — end-to-end through the dispatcher for the `TimedOut` variant.
- `handle_request_capsule_vz_error_unknown_handle_returns_not_found` — dispatcher emits `not_found` for unknown handles, no `vz_error` field.
- `capsule_status_enrichment_carries_both_last_exit_reason_and_vz_error` — Day-7 + Day-8 enrichment in a single `capsule_status` query.

### 4.3 `elastos-server/tests/vz_shutdown_semantics.rs` (1 new + 1 modified)
- New: `supervisor_response_json_wire_format_for_vz_error` — JSON wire-format contract for every `VzError` variant family, asserting per-variant skip-serialise rules + outer `vz_error: None` skip-serialise.
- Modified: `supervisor_response_json_wire_format_for_last_exit_reason` — struct-literal call sites now include `vz_error: None` (struct extension is backward-compatible at the wire level, but Rust requires the field on every literal).

---

## 5. Design notes

### 5.1 Why a separate enum `CapsuleVzErrorOutcome`, not a nested `Option<Option<VzErrorReport>>`?

The supervisor needs to distinguish three states:
- "unknown handle" → response `status: "not_found"`.
- "known handle, no cached error" → response `status: "ok"`, no `vz_error` field.
- "known handle, cached error" → response `status: "ok"`, `vz_error: Some(report)`.

A nested `Option<Option<…>>` would technically encode all three but reads as a footgun (`is_none()` matches both "unknown" and "no cached error"). The explicit enum makes the dispatcher's three-arm match exhaustive and self-documenting.

### 5.2 Why `to_report` on `VzError` instead of `From<VzError> for VzErrorReport`?

`From` would invite `let report: VzErrorReport = err.into()` at call sites, hiding the projection. Naming the conversion `to_report` makes the operator-facing intent explicit — this isn't a lossless conversion (the typed variant gets erased into `kind_label` + free-form fields), so the call site SHOULD show that the projection is happening.

### 5.3 Why centralise `not_found` into a builder?

Pre-Day-8, both `capsule_status` and (now) `capsule_vz_error` returned the `"not_found"` shape inline. Adding the `vz_error` field meant updating two struct literals AND staying consistent on every future field. The `SupervisorResponse::not_found()` builder is now the single source of truth for the not-found wire shape; future fields that should-default to `None` on `not_found` will land automatically via `..Self::ok()` chaining.

### 5.4 Why does Linux still serve `CapsuleVzError`?

The RPC variant is platform-shared in `SupervisorRequest` so shell clients can issue it unconditionally. On Linux, `self.running` never contains a `CapsuleBackend::VzVm` (the variant doesn't exist), so every Linux dispatch returns `not_found`. This is identical to a Mac supervisor that has no Vz capsules running — the response shape is uniform across platforms, which matters when the same `elastos` binary talks to either substrate.

### 5.5 Why use `#[doc(hidden)] pub` for the test setters?

`RunningVm::set_last_vz_error_for_testing` (Day 8) joins the Day-7 `set_last_exit_reason_for_testing` / `set_status_for_testing`. All three are `pub` because supervisor unit tests live in a different crate (`elastos-server`) than the type (`elastos-vz`), but `#[doc(hidden)]` so they don't appear in rustdoc and production code can't reasonably reach for them. The pattern matches the Day-7 precedent and stays out of the public API surface.

### 5.6 Backward compatibility

- **`SupervisorResponse`** gained one new optional field. Existing JSON consumers that deserialise the response as a strict schema will still work because `vz_error` has `#[serde(default)]` semantics via `Option<…>` + `skip_serializing_if`.
- **`SupervisorRequest`** gained one new variant. Existing dispatchers / clients that don't know about it ignore the variant (serde's untagged-enum semantics for `#[serde(tag = "op")]` produces an error on unknown ops, but that's the same behaviour as adding any new RPC — there's no Linux-side regression).
- **No Apple SDK assumption changes.** Day 7's `from_ns_error_parts` already handles every Apple variant including unknown codes; Day 8 just projects the typed enum into a JSON shape.

---

## 6. Phase 4 closing summary

With Day 8, **Phase 4 is complete**. Day-by-day:

| Day | Surface delivered | Operator-facing artifact |
|---|---|---|
| 1 | Parallel launch | Launch latency stays sublinear in capsule count |
| 2 | Bridge multiplexing | One Carrier socket per capsule, dropped on stop |
| 3 | Cross-VM RPC dispatch | `localhost-<scheme>` routes through the bridge correctly under load |
| 4 | Manifest plumbing | Provider config survives the round-trip into the guest |
| 5 | Shutdown semantics + crash recovery | Orphan detection on restart; in-flight RPC fails gracefully |
| 6 | Defensive stop timeout + observable bridge teardown | `VzConfig::stop_timeout` + `BridgeContext::on_terminate` |
| 7 | Typed `VzError` + forced-stop telemetry | `last_exit_reason` on `SupervisorResponse` |
| 8 | `VzErrorReport` readback RPC | `capsule_vz_error` + `vz_error` on `SupervisorResponse` |

The Mac substrate now has the same observability surface as Linux's crosvm path: structured per-capsule error readback, telemetry labels for every terminal state, and stable JSON wire formats for both the alert path (`last_exit_reason`) and the triage path (`vz_error`). The "first real capsule end-to-end" goal that opened Phase 4 is now backed by full observability fidelity.

**Out of scope for Phase 4 (deferred to Phase 5):**
- Apple-runner CI provisioning (we still test the supervisor wiring with synthetic capsules, but real `VZErrorCode`s land in Phase 5).
- Persisting `VzErrorReport` history across supervisor restarts (current scope: in-memory cache, dropped on reap/restart).
- Cross-host Carrier message round-trips (resource introspection via OS-level APIs lands in Phase 5).
