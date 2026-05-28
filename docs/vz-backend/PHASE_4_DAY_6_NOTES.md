## Phase 4 Day 6 — Defensive Vz stop timeout + observable bridge termination

> Outcome log. Status: complete. Day 5 audited the Mac
> teardown graph and surfaced two specific weak spots that the
> "proves the path works" milestone explicitly deferred:
> (a) `VzMachineHandle::stop` had no internal timeout, so a
> wedged `stopWithCompletionHandler:` would pin
> `Supervisor::stop_capsule` indefinitely; (b) the
> `tokio::spawn`ed Carrier-bridge dispatch loop was detached,
> so the supervisor could not *observe* bridge termination,
> only prove (by sending traffic) that it stopped accepting
> data. Day 6 hardens both.

### Defensive stop timeout — design rationale

The Mac substrate is cooperative-only: Apple's
`Virtualization.framework` exposes no `kill -9` equivalent for
`VZVirtualMachine`. The audit from Day 5 noted that if Apple's
completion block fails to fire (framework bug, host paging
stall, OS upgrade regression), the host-side
`tokio::sync::oneshot` inside
`run_completion_handler_on_queue` never resolves and the
supervisor's `stop_capsule` blocks indefinitely. The only
existing bound was the operator's outer RPC-level timeout —
which works for interactive callers but leaves the
supervisor's `running` map permanently inconsistent if
the call is dropped without retry.

Day 6 adds a per-VM stop budget:

* `VzConfig::stop_timeout: Duration` (default
  [`DEFAULT_VZ_STOP_TIMEOUT`](../../elastos/crates/elastos-vz/src/config.rs)
  = 30 s). Linux's `CrosvmConfig` has no analogue — its
  `RunningVm::stop` uses SIGTERM + 5 s SIGKILL escalation, a
  preemptive mechanism Vz lacks.
* `VzMachineHandle` stores the timeout, plumbed through
  `VzMachineHandle::new` from `VzProvider::load_with_vm_config`.
* `VzMachineHandle::stop` wraps the inner completion-handler
  future with `tokio::time::timeout`. On timeout, returns a
  typed `Err(String)` whose message names the budget, the
  vm_id, and points at this document for the operator
  runbook.

#### Why 30 s default

Vz's documented `stopWithCompletionHandler:` delay covers:
* Guest kernel shutdown sequencing (`init 0`, paravirt device
  drain).
* Vz-internal cleanup (VMM thread join, file-backed disk image
  sync).

In practice Apple's framework resolves the completion in
sub-second on idle hardware. The 30 s upper bound is generous
enough that no production stop should hit it, short enough
that a wedged framework call surfaces a typed error within
the operator's attention window. Operators on slow / debug
hardware can extend it via `VzConfig::with_stop_timeout`; CI
uses short values (e.g. 100 ms) to exercise the timeout path
deterministically.

#### Why best-effort cleanup on timeout

When the timeout fires, the supervisor cannot prove the Vz
handle has actually stopped — Apple's completion may yet fire
asynchronously, or it may never fire. Two failure modes are
possible:
1. The Vz handle is already stopped and Apple just didn't
   signal us. The remaining cleanup (overlay file removal,
   bridge drain) is safe and operator-observable as a clean
   stop.
2. The Vz handle is still running. Process exit will kill it
   (Vz instances die with the owning process). The supervisor
   has no in-process recovery option — there's no public Vz
   API to force-terminate a wedged VM.

In both cases the right move is the same: log the timeout at
WARN level, run all the local cleanup that doesn't depend on
the Vz handle being stopped (overlay removal, provider-route
unregistration, bridge wait), and surface a typed error to
the caller. `stop_capsule` therefore returns
`Err("Vz VM stop failed for 'X': ... (cleanup ran best-effort)")`
on timeout — operator sees the failure but the supervisor's
local state is consistent so the next `launch_capsule` of the
same name is not blocked by stale on-disk artifacts.

#### `DelegateExit::ForcedAfterTimeout`

The Day 5 audit also noted that
`VzMachineHandle::wait_for_exit` would hang if `stop` timed
out without signalling the delegate's exit channel.
Introduced new variant `DelegateExit::ForcedAfterTimeout`
with exit code **137** (matches Linux's `128 + SIGKILL(9)`
convention for forcibly-killed processes). Now any concurrent
`wait_for_exit` waiter resolves with 137 on timeout — same
operator-facing exit code semantics across substrates.

| `DelegateExit` variant | Source | Exit code |
|---|---|---|
| `GuestCleanStop` | Delegate: guest issued `poweroff -h` / `init 0` | 0 |
| `HostInitiatedStop` | `VzMachineHandle::stop` resolved Ok | 0 |
| `StoppedWithError` | Delegate: `virtualMachine:didStopWithError:` | 1 |
| **`ForcedAfterTimeout`** (Day 6) | `VzMachineHandle::stop` hit `stop_timeout` | **137** |

### Observable bridge termination — design

The Day 5 audit established that the
`tokio::spawn`ed `run_carrier_bridge_loop` task exits when
its `UnixStream` reaches EOF — but the supervisor had no way
to *observe* that exit. The only proof-by-traffic was
indirect: send a request and watch it fail.

Day 6 adds an opt-in observer:

* `BridgeContext::on_terminate: Option<Arc<tokio::sync::Notify>>`.
* When set, `run_carrier_bridge_loop` calls
  `notify.notify_waiters()` on EVERY exit path (EOF, read
  error, write error) before returning.
* Existing call sites (Linux crosvm bridge, WASM stdio
  bridge) keep `on_terminate: None` — bridge lifecycle
  observation is a Mac/Vz-specific facility today.

The supervisor's Mac launch path
(`start_capsule_vm_macos`) mints a fresh `Arc<Notify>`,
embeds it in the `BridgeContext`, then extracts the
clone into `RunningCapsule::bridge_terminated`. After
`vm.stop().await` resolves, `stop_capsule` awaits
`notify.notified()` with a 10 s budget. The whole sequence:

1. `vm.stop()` — returns `Ok(())` on clean stop, or
   `Err(Timeout)` on stuck completion.
2. If `bridge_terminated.is_some()`, await `notify.notified()`
   with `tokio::time::timeout(10s, ...)`.
   * `Ok(())` → log `tracing::debug!` "bridge terminated cleanly".
   * `Err(Elapsed)` → log `tracing::warn!` "bridge orphaned —
     continuing best-effort".
3. Always run overlay cleanup.
4. If `vm.stop()` returned `Err`, surface it now (after
   local state has been cleaned up).

#### Why not store the `JoinHandle`?

Could have `tokio::spawn(...).await`ed the bridge directly.
Two reasons not to:

1. **Encapsulation.** `JoinHandle` exposes the bridge's task
   identity to the supervisor; the supervisor only needs
   "did it exit?", not "join the future." `Arc<Notify>` is a
   pure observability primitive.

2. **Existing detached-spawn pattern.** All three
   `spawn_carrier_bridge_on_stream` call sites
   (`run_carrier_bridge_loop`, the WASM bridge, the
   `spawn_carrier_bridge` listener path) detach. Retaining
   `JoinHandle`s only on the Mac path would create a
   per-platform divergence inside `BridgeContext`'s API
   surface; the `Option<Arc<Notify>>` keeps the divergence
   limited to one optional field.

#### Why 10 s bridge-wait budget

* The Day 5 audit observed sub-second NSFileHandle release
  lag between `VZVirtualMachine.stop:` resolving and the
  carrier pipe write fd closing.
* 10 s comfortably covers the worst-case observed on dev
  hardware (~50 ms) plus an order of magnitude headroom.
* Aligned with the 10 s budget the Day 4 smoke test uses
  for `wait_for_stopped` polling so operators see a
  consistent timeout boundary across supervisor RPCs.

### Test inventory

```
elastos-vz/src/ffi/lifecycle.rs (lib tests)
  ffi::lifecycle::tests::drive_stop_with_timeout_returns_typed_error_when_inner_future_never_resolves
  ffi::lifecycle::tests::drive_stop_with_timeout_passes_through_ok_when_inner_future_resolves_first
  ffi::lifecycle::tests::drive_stop_with_timeout_classifies_apple_error_distinctly_from_timeout

elastos-vz/src/ffi/delegate.rs (lib test)
  ffi::delegate::tests::delegate_exit_maps_to_expected_codes   (extended for ForcedAfterTimeout = 137)

elastos-server/src/supervisor.rs (lib tests)
  supervisor::tests::stop_capsule_proceeds_immediately_when_bridge_termination_notify_fires
  supervisor::tests::stop_capsule_does_not_block_when_bridge_terminated_is_none

elastos-server/tests/vz_shutdown_semantics.rs (integration tests)
  bridge_on_terminate_notify_fires_when_dispatch_loop_exits_on_eof
  bridge_on_terminate_notify_is_observable_when_subscribed_before_teardown
```

Test budgets:

* `drive_stop_with_timeout` tests use `tokio::test(start_paused = true)`
  for the never-resolving case so virtual time advances
  instantly — total wall-clock < 2 s.
* `stop_capsule_proceeds_immediately_when_bridge_termination_notify_fires`
  asserts < 500 ms elapsed wall-clock. 1 s outer timeout.
* `stop_capsule_does_not_block_when_bridge_terminated_is_none`
  asserts < 500 ms elapsed wall-clock. No outer timeout (the
  `None` path takes zero notify-wait time by construction).
* Bridge-observability tests assert `notify.notified()` fires
  within 1 s of stream drop.

All tests pass under `RUST_TEST_THREADS=1` and `=4`.

### CI gates (Day 6 acceptance)

* `cargo fmt --all -- --check` — clean.
* `cargo clippy --workspace --all-targets -- -D warnings` —
  clean on Mac and Linux.
* `RUST_TEST_THREADS=1 cargo test -p elastos-server` — green.
* `RUST_TEST_THREADS=4 cargo test -p elastos-server` — green.
* `RUST_TEST_THREADS=1 cargo test -p elastos-vz` — green.
* `scripts/check-linux-untouched.sh bcf5a0a` — green. Touched
  files are limited to:
  * `elastos-vz/src/config.rs` (new field + ctor + helper),
  * `elastos-vz/src/lib.rs` (re-export),
  * `elastos-vz/src/ffi/delegate.rs` (new variant + test),
  * `elastos-vz/src/ffi/lifecycle.rs` (timeout wrapping + tests),
  * `elastos-vz/src/provider.rs` (one line to plumb the budget),
  * `elastos-server/src/carrier_bridge.rs` (optional Notify),
  * `elastos-server/src/supervisor.rs` (Mac-only call sites),
  * `elastos-server/src/runtime.rs` (one line for the WASM ctor),
  * `elastos-server/tests/vz_shutdown_semantics.rs` (two tests).
  All protected crates (`elastos-crosvm`, `elastos-runtime`,
  `elastos-common`, `elastos-compute`) untouched.

### Forward pointers (Phase 4 Day 7+)

* Typed `VZErrorCode` → `ElastosError` variant mapping (per the
  Day 5 failure-mode matrix). The `StopError::Apple` branch in
  this commit is the place that mapping would land.
* Optional `Supervisor::new_and_prune` constructor for
  single-instance deployments that want both Day 5's startup
  cleanup AND a chosen `stop_timeout`.
* Apple-runner CI provisioning (out of scope until parity gate
  is fully closed).
* Surface `DelegateExit::ForcedAfterTimeout` (exit 137) in the
  `elastos status` JSON so operators / dashboards can flag
  forced stops separately from clean ones.
