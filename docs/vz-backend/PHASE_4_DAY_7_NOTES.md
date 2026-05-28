# Phase 4 Day 7 — Typed `VzError` mapping + forced-stop telemetry surface

> **Status:** Complete. Closes Phase 4's observability-fidelity gap that the Day 5 audit flagged ("the supervisor sees Vz errors as string-formatted `ElastosError::Compute` instances — fine for logs, useless for structured telemetry"). With Day 7, every Vz failure routes through a typed enum and every forced-stop event surfaces as a canonical telemetry label on `SupervisorResponse`.
>
> **Linux-untouched gate:** `scripts/check-linux-untouched.sh bcf5a0a` green. The new `VzError` lives in `elastos-vz`; `ElastosError` (in the protected `elastos-common` crate) is unchanged. The supervisor reads `RunningVm::last_exit_reason()` directly — no protected-crate change required.
>
> **Day 6 anchors:** [`PHASE_4_DAY_6_NOTES.md`](PHASE_4_DAY_6_NOTES.md).
> **Day 5 anchors:** [`PHASE_4_DAY_5_NOTES.md`](PHASE_4_DAY_5_NOTES.md) (failure-mode matrix).

---

## 1. What Day 7 ships

| Component | File | Change |
|---|---|---|
| `VzError` enum + classifier | `elastos/crates/elastos-vz/src/error.rs` (new) | Typed enum mirroring `VZErrorCode` + synthetic `TimedOut` + forward-compat `Unknown`. `from_ns_error_parts(domain, code, description)` does the classification. |
| `VzExitReason` enum | `elastos/crates/elastos-vz/src/error.rs` (new) | Canonical exit-code + telemetry-label source of truth (`guest_clean_stop` / `host_initiated_stop` / `stopped_with_error` / `forced_after_timeout`). |
| FFI typed plumbing | `elastos/crates/elastos-vz/src/ffi/lifecycle.rs` | `VzMachineHandle::start` / `stop` return `Result<(), VzError>`; `run_completion_handler_on_queue` reads `NSError.domain` / `.code` / `.localizedDescription` and routes the typed variant. `wait_for_exit_classified` returns `Result<VzExitReason, String>`. |
| Delegate de-duplication | `elastos/crates/elastos-vz/src/ffi/delegate.rs` | `DelegateExit::exit_code()` removed — the canonical mapping now lives in `VzExitReason::exit_code()`. `DelegateExit` stays FFI-internal. |
| Public re-exports | `elastos/crates/elastos-vz/src/lib.rs` | `pub use error::{VzError, VzExitReason}`. |
| `RunningVm` cached telemetry | `elastos/crates/elastos-vz/src/vm.rs` | New `last_vz_error: Option<VzError>` and `last_exit_reason: Option<VzExitReason>` fields, populated on every `stop` / `wait_for_exit_code`. Public accessors + `#[doc(hidden)]` test setters. |
| Supervisor wire format | `elastos/crates/elastos-server/src/supervisor.rs` | `SupervisorResponse::last_exit_reason: Option<String>` (`skip_serializing_if = "Option::is_none"`). `stop_capsule` returns `Result<Option<String>>`; the dispatcher's `StopCapsule` arm uses `ok_with_exit_reason`. `capsule_status` populates the field from `vz_last_exit_reason(&backend)`. |
| Tests | `elastos-vz/src/error.rs`, `elastos-vz/src/ffi/lifecycle.rs`, `elastos-server/src/supervisor.rs`, `elastos-server/tests/vz_shutdown_semantics.rs` | See §4 inventory. |

---

## 2. Typed-error mapping table

### 2.1 `VZErrorCode` → `VzError` variants

The `from_ns_error_parts(domain, code, description)` helper classifies every Apple `NSError` raised on the Vz lifecycle path. Only codes where the `domain` matches `"VZErrorDomain"` (case-insensitive) are recognised as typed; others surface as `Unknown` with the original domain preserved so logs / telemetry don't lose information.

| Apple symbol | `VZErrorCode` value | `VzError` variant | `kind_label` (telemetry) | When typically seen |
|---|---|---|---|---|
| `VZErrorInternal` | 1 | `Internal { description }` | `vz_internal` | Generic framework-level failure (rare; possible mid-stop if Apple's stop handler hits an internal issue). |
| `VZErrorInvalidVirtualMachineConfiguration` | 2 | `InvalidConfiguration { description }` | `vz_invalid_configuration` | `validateWithError` rejected the config (memory > host, missing entitlement, bad rootfs). |
| `VZErrorInvalidVirtualMachineState` | 3 | `InvalidState { description }` | `vz_invalid_state` | `stop` called before `start`, etc. |
| `VZErrorInvalidVirtualMachineStateTransition` | 4 | `InvalidStateTransition { description }` | `vz_invalid_state_transition` | Apple rejected a state change as illegal. |
| `VZErrorNetworkError` | 7 | `NetworkError { description }` | `vz_network_error` | Network attachment failure during start / stop. |
| `VZErrorOperationCancelled` | 9 | `OperationCancelled { description }` | `vz_operation_cancelled` | Apple cancelled the operation (e.g. `stop` issued while still `Starting`). Day 5 failure-mode item. |
| `VZErrorNotSupported` | 10 | `NotSupported { description }` | `vz_not_supported` | Operation not available on this host (entitlement, OS version, hardware). |
| *(synthetic — no Apple code)* | — | `TimedOut { vm_id, budget }` | `vz_timed_out` | Day 6 stop-timeout fired before Apple's completion handler. Vz handle is best-effort orphaned. |
| *(any unmodelled code on `VZErrorDomain`)* | * | `Unknown { domain, code, description }` | `vz_unknown` | Future Apple variant or a code we haven't classified yet. |
| *(any code on a non-`VZErrorDomain`)* | * | `Unknown { domain, code, description }` | `vz_unknown` | Lower-level OS error surfacing through Vz (e.g. `NSPOSIXErrorDomain`). Domain is preserved. |

### 2.2 `VzExitReason` → exit code + telemetry label

| Variant | `label()` | `exit_code()` | Origin |
|---|---|---|---|
| `GuestCleanStop` | `guest_clean_stop` | 0 | Apple's delegate fired `guestDidStopVirtualMachine:` (`poweroff -h`, `init 0`). |
| `HostInitiatedStop` | `host_initiated_stop` | 0 | `VzMachineHandle::stop` resolved cleanly — supervisor asked, Apple confirmed. |
| `StoppedWithError` | `stopped_with_error` | 1 | Apple's delegate fired `virtualMachine:didStopWithError:` — VM tore down on a framework-level fault. |
| `ForcedAfterTimeout` | `forced_after_timeout` | 137 | Day 6 stop-timeout: Apple's completion never fired within `VzConfig::stop_timeout`. Mirrors Linux's `128 + SIGKILL(9) = 137`. |

These labels are the **stable telemetry contract** dashboards consume. Adding a new variant must update this table.

---

## 3. JSON schema diff — `SupervisorResponse`

### 3.1 Before Day 7

```jsonc
{
  "status": "ok",
  "handle": "vm-shell-aaa-0",
  "vsock_cid": 1234,
  "uptime_secs": 42
  // … other optional fields …
}
```

### 3.2 After Day 7

```jsonc
{
  "status": "ok",
  "handle": "vm-shell-aaa-0",
  "vsock_cid": 1234,
  "uptime_secs": 42,
  "last_exit_reason": "forced_after_timeout"   // ← NEW (Phase 4 Day 7)
}
```

Notes:

- The new field is `Option<String>` with `#[serde(skip_serializing_if = "Option::is_none")]`. Legacy dashboards / scripts that don't recognise the field keep working unchanged.
- Populated only for `stop_capsule` and `capsule_status` responses against macOS Vz capsules. Linux crosvm, Carrier services, and Vz capsules with no cached outcome surface `None` (i.e. the key is omitted).
- Possible label values are exactly the four `VzExitReason` labels listed in §2.2.

### 3.3 Operator alerting recipes

**Datadog monitor — forced-stop rate above 1/hour:**
```
sum:elastos.stop_response{status:ok,last_exit_reason:forced_after_timeout}.as_count().rollup(sum, 3600) > 1
```

**Grafana panel — exit-reason distribution over time:**
```promql
sum by (last_exit_reason) (rate(elastos_stop_count{}[5m]))
```

(Field names are illustrative — the JSON contract is the wire-format truth; the metric pipeline is operator-side.)

---

## 4. Test inventory

### 4.1 Pure-classifier tests — `elastos/crates/elastos-vz/src/error.rs`

| Test | Contract |
|---|---|
| `from_ns_error_parts_maps_documented_vzerror_codes_to_typed_variants` | Each of `VZErrorCode::Internal` / `InvalidConfiguration` / `InvalidState` / `InvalidStateTransition` / `NetworkError` / `OperationCancelled` / `NotSupported` maps to the expected typed variant + `kind_label`. Description round-trips via `NSError.localizedDescription`. |
| `from_ns_error_parts_maps_unknown_vz_codes_to_unknown_variant` | Codes Apple may add in future macOS revisions (e.g. 99999) surface as `Unknown` with original code preserved — no information loss. |
| `from_ns_error_parts_routes_non_vz_domain_to_unknown_preserving_domain` | `NSError`s from lower-level domains (e.g. `NSPOSIXErrorDomain`) classify as `Unknown` with the domain string preserved. Case-insensitive matching on `VZErrorDomain`. |
| `timed_out_description_embeds_vm_id_budget_and_runbook_pointer` | `VzError::TimedOut` description includes vm_id (log correlation), budget (operator can confirm configured value), and `PHASE_4_DAY_6_NOTES.md` (runbook pointer). |
| `vz_exit_reason_labels_and_exit_codes_are_stable` | Pins every label string + exit code — dashboards depend on these as stable strings. |
| `vz_error_display_includes_kind_label_for_log_grep` | `Display` prefixes with `kind_label` so `grep` finds `vz_internal:` etc. in logs without ambiguity. |

### 4.2 FFI plumbing tests — `elastos/crates/elastos-vz/src/ffi/lifecycle.rs`

| Test | Contract |
|---|---|
| `drive_stop_with_timeout_returns_typed_error_when_inner_future_never_resolves` | Updated for Day 7: now expects `VzError::TimedOut { vm_id, budget }` instead of the old internal `StopError::Timeout(String)`. |
| `drive_stop_with_timeout_passes_through_ok_when_inner_future_resolves_first` | Nominal path: typed `Ok(())` passes through unchanged. |
| `drive_stop_with_timeout_classifies_apple_error_distinctly_from_timeout` | Apple `NSError`-derived `VzError::InvalidState` passes through distinct from `TimedOut` — supervisor uses the distinction to choose `HostInitiatedStop` vs `ForcedAfterTimeout`. |
| `delegate_exit_to_reason_classifies_every_variant` | New: pins the FFI→public mapping for every `DelegateExit` variant including the `StoppedWithError(String)` payload arm. |

### 4.3 Supervisor tests — `elastos/crates/elastos-server/src/supervisor.rs`

| Test | Contract |
|---|---|
| `capsule_status_includes_last_exit_reason_for_forced_after_timeout_vz_capsule` | `capsule_status` JSON includes `last_exit_reason: Some("forced_after_timeout")` after the synthetic capsule's `RunningVm::set_last_exit_reason_for_testing(ForcedAfterTimeout)` is set. |
| `capsule_status_round_trips_every_vz_exit_reason_label` | Every `VzExitReason` variant round-trips through `capsule_status`'s `last_exit_reason` field. Regression guard for new variants. |
| `capsule_status_omits_last_exit_reason_when_vz_capsule_has_no_cached_outcome` | Vz capsules with no cached outcome surface `None` — backward-compat with legacy dashboards. |
| `capsule_status_not_found_response_has_no_last_exit_reason` | `not_found` responses never carry an exit reason. |
| `stop_capsule_removes_vz_vm_from_running_map` | Updated for Day 7: now asserts the synthetic-no-handle path returns `None` for `last_exit_reason` (only the no-op `RunningVm::stop` ran). |
| `handle_request_stop_capsule_surfaces_typed_last_exit_reason_in_response` | End-to-end through the dispatcher: `handle_request(StopCapsule)` for a synthetic capsule with cached `ForcedAfterTimeout` returns `SupervisorResponse { status: "ok", last_exit_reason: Some("forced_after_timeout") }`. |

### 4.4 Integration test — `elastos/crates/elastos-server/tests/vz_shutdown_semantics.rs`

| Test | Contract |
|---|---|
| `supervisor_response_json_wire_format_for_last_exit_reason` | JSON serialisation invariants: every `VzExitReason::label()` round-trips through `serde_json::to_string`; `None` skip-serialises (no `last_exit_reason` key) so legacy dashboards keep working unchanged. |

---

## 5. Design notes

### 5.1 Why `VzError` and not `ElastosError::Vz(VzError)`

`ElastosError` lives in `elastos-common`, which is on the Linux-untouched gate. Adding a variant there would change the protected crate. Day 7's chosen path keeps `ElastosError::Compute(String)` as the trait-boundary surface (so the `ComputeProvider` trait signature is unchanged) and exposes the typed flavour two ways:

1. **`VzError::Display`** prefixes with `kind_label`, so even consumers who only see the `Compute(String)` arm get a grep-friendly label.
2. **`RunningVm::last_vz_error()`** + **`RunningVm::last_exit_reason()`** — supervisor reads the cached typed surface directly off the `RunningVm` after `stop` returns.

This satisfies the prompt's escape hatch: *"if `ElastosError` is in a protected crate, surface the typed Vz error via a downcast helper or a new `elastos-vz`-side error trait instead of touching the protected crate."*

### 5.2 Why `DelegateExit::exit_code()` was removed

Day 6 introduced `DelegateExit::exit_code()`. Day 7 makes `VzExitReason::exit_code()` the canonical mapping (consumed by `wait_for_exit_classified`). Keeping both would invite drift — the `delegate_exit_maps_to_expected_codes` test guards the contract from inside `VzExitReason`'s tests now, and the FFI→public conversion is covered by the new `delegate_exit_to_reason_classifies_every_variant` test.

### 5.3 Why the test hooks are `#[doc(hidden)] pub`

Real `VzExitReason::ForcedAfterTimeout` requires a wedged Apple completion handler — impossible to provoke without an Apple-runner CI (Phase 5 deliverable). The supervisor's wiring (which is what Day 7 changes) is what the unit tests need to validate. `RunningVm::set_last_exit_reason_for_testing` and `set_status_for_testing` are `#[doc(hidden)]` so they don't appear in the public API; they are deliberately *not* `#[cfg(test)]` because cross-crate `#[cfg(test)]` doesn't expose symbols.

### 5.4 Backward compatibility

- `SupervisorResponse::last_exit_reason: Option<String>` with `skip_serializing_if = "Option::is_none"` — legacy dashboards see the same JSON they did before for non-Vz / no-cached-reason responses.
- `RunningVm::stop` signature unchanged (`Result<()>`). The typed surface is additive via `RunningVm::last_vz_error()` / `last_exit_reason()`.
- `ComputeProvider` trait surface unchanged — `VzProvider` continues to implement `stop(&self) -> Result<(), ElastosError>` by converting `VzError → ElastosError::Compute(format)` at the trait boundary, with `Display` preserving the kind_label.

---

## 6. What Day 7 deliberately does NOT do

- **Apple-runner CI provisioning.** Phase 5 Day 1+ deliverable.
- **Cross-host Carrier message round-trips.** Phase 5.
- **Resource-leak detection via OS-level introspection.** Phase 5.
- **Backport `VzError` typed surface to Linux error codes.** Out of scope — Linux path stays on its existing crosvm-style string errors.
- **Variant for `VZErrorVirtualMachineGuestPaniced`.** The `objc2-virtualization-0.3.2` binding does not expose a constant for this code (likely added in macOS 15+ headers; not yet in the binding). When a guest panic surfaces today it routes through `VzError::Unknown { code: <whatever Apple sends>, … }`. This is exactly the forward-compatibility shape `Unknown` exists for — when the binding updates, we add a typed variant and the `Unknown` fallback becomes a regression guard.

---

## 7. Operator-facing contract summary

After Day 7, the following invariants hold for the Vz substrate:

1. **No `unwrap` / `panic` on any Vz lifecycle error.** Every `NSError` Apple raises routes through `VzError` and either reaches the supervisor as a structured `last_vz_error()` (typed) + a `Compute(format!("{kind_label}: {description}"))` (string) for logs, or — for `TimedOut` — propagates through Day 6's best-effort cleanup path.

2. **Every successful stop publishes a telemetry label.** `SupervisorResponse::last_exit_reason` carries one of the four canonical labels for a stopped Vz capsule; legacy dashboards see the same JSON they did before for everything else.

3. **`forced_after_timeout` is the alertable forced-stop signal.** It appears exactly when Day 6's stop-timeout fired — i.e. when Apple's completion handler never resolved within `VzConfig::stop_timeout` (default 30 s) and the Vz handle was best-effort orphaned.

4. **The typed surface is forward-compatible.** Apple adding a new `VZErrorCode` in a future macOS surfaces as `VzError::Unknown { code, description, domain: "VZErrorDomain" }`. The `kind_label` is `vz_unknown`. Logs / dashboards still get the original code + description so the operator can recognise it before we update the enum.

---

## 8. Day 8+ / Phase 5 follow-ups

- Apple-runner CI provisioning so the typed surface is exercised against real Vz errors (today the integration tests exercise the *wire format* not Apple's actual NSError shapes).
- Add a `VZErrorVirtualMachineGuestPaniced` typed variant once the `objc2-virtualization` binding updates.
- Consider a `RunningVm::last_vz_error_kind_label()` shortcut to avoid the `error().to_string()` / `kind_label()` ceremony in supervisor logs.
- Promote `last_vz_error` to a queryable supervisor RPC so operators can fetch the structured error without parsing the `error` string field.

---

## 9. Cross-references

- Day 5 audit: [`PHASE_4_DAY_5_NOTES.md`](PHASE_4_DAY_5_NOTES.md) (failure-mode matrix that drove `VzError`'s variant set).
- Day 6 stop-timeout runbook: [`PHASE_4_DAY_6_NOTES.md`](PHASE_4_DAY_6_NOTES.md) (`VzError::TimedOut` description points operators here).
- `state.md` L88, L92–94 — Mac as a truthful first-class target requires this kind of structured observability before Phase 5 hardening can claim parity.
- `PRINCIPLES.md` #11 *Fail Closed, Then Explain* — the typed `kind_label` + structured `last_exit_reason` are the "explain" half of the principle made operator-grade.
