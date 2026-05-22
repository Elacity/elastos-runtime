## Phase 4 Day 5 — Shutdown semantics + crash-recovery audit

> Outcome log. Status: complete. Days 1–4 proved the forward path
> for the Mac substrate (parallel launch, multiplexed Carrier
> bridges, cross-VM RPC dispatch, manifest plumbing parity). Day
> 5 closes the *messy* path: how the system behaves when a VM
> stops mid-flight, when the supervisor process dies without
> calling `stop_capsule`, and which Vz failure modes the rest of
> the runtime must be ready to surface.

### Shutdown sequence audit

The Mac teardown graph for a MicroVM capsule has five layers,
each with a single responsibility. The audit below traces an
operator-initiated stop end-to-end and notes the surface
differences from Linux/crosvm.

| Layer | Mac behaviour | Linux behaviour | Notes |
|---|---|---|---|
| 1. `Supervisor::stop_capsule(handle)` | Acquires `running.write()`, removes the entry, drops the `running` lock, then matches on `CapsuleBackend::VzVm` and calls `vm.stop().await`. Removes the `<handle>.ext4` overlay on success. | Same shape; matches `CapsuleBackend::Vm` and calls `vm.stop().await`. Removes the same overlay. | The `unregister_provider_route` call happens BEFORE the platform branch on both paths, so cross-VM RPCs targeting the capsule's scheme start returning `NoProvider` immediately. |
| 2. `RunningVm::stop` (`elastos-vz/src/vm.rs`) | If a `VzMachineHandle` is attached, dispatches `handle.stop().await`. Sets `self.status = Stopped`. Idempotent (no-op when no handle). | Sends SIGTERM to the crosvm child, then `kill -9` after 5 s. | Mac path is fully cooperative; Linux path is preemptive on timeout. |
| 3. `VzMachineHandle::stop` (`elastos-vz/src/ffi/lifecycle.rs`) | Dispatches `VZVirtualMachine.stopWithCompletionHandler:` on the per-VM `VzDispatchQueue` via `run_completion_handler_on_queue`. Awaits a `tokio::sync::oneshot`. On success, signals `exit_state` with `HostInitiatedStop` so any concurrent `wait_for_exit` waiter resolves. | N/A (no Mach equivalent). | Apple guarantees the completion block fires exactly once. The block runs on the VM's queue, so the oneshot's `send()` is dispatched-safe. |
| 4. Drop chain after stop | `RunningCapsule.backend` falls out of scope → `Box<RunningVm>` drops → `Option<VzMachineHandle>` drops → `Arc<VZVirtualMachine>` ref count → 0 → Vz framework dealloc → `VZVirtualMachineConfiguration` releases → `NSFileHandle` (console pipe) deallocs → host-side pipe write fd closes. | `RunningVm` drops → child PID `wait()` completes → all child fds collected by the kernel. | The Vz drop chain is **asynchronous and indirect**. The host-side carrier socket fd doesn't EOF until Vz's NSFileHandle releases, which can lag the `stop` completion by milliseconds. The bridge loop's `read_line` returns `Ok(0)` only after that release. |
| 5. Bridge task exit | `run_carrier_bridge_loop`'s `BufReader::read_line` returns 0 → `break` → loop ends → `tokio::spawn`ed task completes. | Same. | The `JoinHandle` is detached on both platforms; Day 5's test re-verifies that detachment is safe (no leak even under repeated start/stop cycles). |

Key surface deltas vs Linux:

* **Cooperative-only stop.** Mac has no equivalent of `kill -9`
  on a Vz VM — `VZVirtualMachine.stop:` is the only documented
  termination API. If Apple's completion never fires, the
  `tokio::sync::oneshot` never resolves and `RunningVm::stop`
  hangs. Today this is bounded only by the operator's outer
  timeout (the supervisor's RPC-level timeout, not a Vz-level
  one). **Day 6+ ticket:** add a defensive `tokio::time::timeout`
  inside `VzMachineHandle::stop` that returns a typed error
  rather than blocking the supervisor indefinitely.

* **Pipe-release race window.** Between `VZVirtualMachine.stop:`
  resolving and the underlying NSFileHandle deallocating, the
  carrier bridge's host fd is still open. A `send_raw` issued
  during that window can succeed-then-fail (writes buffered,
  reads stall, eventual POLLHUP). Day 5's `closing_host_side_of_carrier_socket_terminates_bridge_loop_within_one_second`
  test bounds the observable window to <1s on dev hardware; the
  graceful-failure test covers the in-flight read case.

* **No host-side child to reap.** Linux has `wait_for_exit` that
  blocks on the crosvm PID. Mac's `wait_for_exit` polls Vz's
  `state` property (Phase 3 Day 5 replaced this with the
  delegate's oneshot). There's no zombie process risk because
  Vz instances die with the supervisor process — they cannot
  leak across process boundaries (verified by inspection of
  Apple's `Virtualization.framework` headers; there is no
  `VZAttach…` or similar mechanism for cross-process VM
  control).

### In-flight cross-VM RPC graceful failure

Added `elastos-server/tests/vz_shutdown_semantics.rs::in_flight_cross_vm_rpc_surfaces_provider_error_when_target_vm_stops`.

Fixture pattern (re-using the Phase 3 Day 6 / Phase 4 Day 3
socketpair pattern):

* Two `socketpair(AF_UNIX, SOCK_STREAM)` halves: host + guest.
* Synthetic guest thread ACKs the bridge's `init` line, reads
  the first request, signals `request_observed`, then sits in a
  blocking read forever — modelling a stalled guest.
* `VmCapsuleProvider::new_with_vsock_dialer` hands the host fd to
  the bridge via a one-shot `MacVsockDial`.
* Consumer task: `registry.send_raw("localhost-stalled", {…})`.

Stop simulation: the test joins the synthetic VM thread (which
drops the guest stream), modelling the observable outcome of
`VZVirtualMachine.stop` releasing the carrier pipe. The host
bridge's `poll()` then sees `POLLHUP`.

Expected outcome (and the test's contract): consumer's
`send_raw` returns a typed `ProviderError` whose message
includes one of:

* `"unhealthy"` — the `POLLHUP` path through
  `VmRawBridge::wait_for_readable`.
* `"closed"` — EOF on a mid-read.
* `"timed out"` — the bridge's 15 s read timeout if neither of
  the above fires first.

All three are acceptable graceful surfaces; what is NOT
acceptable is silent `Ok`, infinite block, or panic. The test
asserts within a 30 s budget; observed runtime on dev hardware
is ~15 s (the bridge's natural read timeout dominates because
the synthetic VM holds the guest fd until thread join).

### Carrier-bridge task lifecycle audit

Day 2's unit test (`dropping_one_carrier_endpoint_terminates_only_that_bridge`)
already proved per-bridge isolation in isolation. Day 5 adds
the integration-level mirror:

* `elastos-server/tests/vz_shutdown_semantics.rs::closing_host_side_of_carrier_socket_terminates_bridge_loop_within_one_second`
* Drives `spawn_carrier_bridge_on_stream` directly with a Tokio
  socketpair, drops the guest stream, immediately spins up a
  fresh bridge on a *new* socketpair, asserts the second bridge
  becomes responsive in <1 s.

The detached-spawn pattern (`tokio::spawn` without storing the
`JoinHandle`) is acceptable here because:

1. The bridge is purely reactive (no host-side ticker, no
   shared mutable state beyond the `ProviderRegistry` Arc).
2. Loop exit conditions are exhaustively bounded:
   * `Ok(0)` from `read_line` → EOF.
   * `Err(_)` from `read_line` → I/O failure.
   * `write_all(…).is_err()` → broken pipe.
   * Oversized line → write the framed error, `continue`.
3. Tokio detaches `JoinHandle`s on drop without resource leak;
   the only state the task holds (the `BufReader<OwnedReadHalf>`
   and `OwnedWriteHalf`) drops with the task frame.

If a future ticket needs to observe bridge termination from the
supervisor (e.g. for metrics), the right surface is a
`tokio::sync::Notify` passed in via `BridgeContext`, not
retaining the `JoinHandle`. Documented here for posterity.

### Orphan VM detection on restart

Added `prune_stale_mac_artifacts` (free fn in
`elastos-server/src/supervisor.rs`) + `Supervisor::prune_stale_mac_artifacts`
method.

Scope of cleanup:

* `<rootfs_cache_dir>/overlays/*.ext4` — orphaned writable
  rootfs overlays from a crashed supervisor.
* `<socket_dir>/*-carrier.sock`, `<socket_dir>/*.sock` —
  orphaned Carrier IPC sockets + crosvm control sockets. (The
  Mac path doesn't create the `.sock` files today, but the
  helper still cleans them for forward-compatibility with the
  shared Linux/Mac socket_dir.)

What is **NOT** cleaned:

* Non-`.ext4` files in `overlays/` (operator notes, config).
* Non-socket files in `socket_dir/` (config files, logs).

This keeps the helper from doubling as a wildcard `rm -rf` on
the user's data dir — the unit test
`prune_stale_mac_artifacts_removes_overlays_and_sockets_but_preserves_unrelated_files`
asserts both directions of the contract (artifacts removed,
unrelated files preserved, second sweep is a no-op).

Why NOT wired into `Supervisor::new` automatically:

* Two concurrent supervisor processes targeting the same
  `data_dir` would nuke each other's in-flight overlays.
* `Supervisor::new` is a constructor; mutating filesystem state
  inside a constructor violates least-surprise.
* Operators (or future `elastos serve` startup glue) call the
  method explicitly when they know no other supervisor is
  running.

Linux behaviour: `Supervisor::prune_stale_mac_artifacts` is a
no-op stub returning `StaleArtifactCounts::default()`. The
Linux path's existing `reap_dead_capsules` (timer-driven)
covers the equivalent surface for crosvm child processes.
Operators don't need to call this on Linux.

Also added `fresh_supervisor_does_not_falsely_report_stale_overlay_files_as_running`:
constructs a `Supervisor` against a data dir pre-populated
with ghost overlay + socket files, asserts that `running` is
empty (the supervisor doesn't scan the filesystem to populate
its state) and then asserts the opt-in cleanup removes the
ghosts.

### Vz teardown failure-mode matrix

Apple's `VZVirtualMachine.stopWithCompletionHandler:` resolves
either with a nil `NSError` (success) or a non-nil `NSError`
whose `code` is drawn from `VZErrorCode`. The relevant codes
during stop, with the current host-side handling:

| `VZErrorCode` (Apple) | Trigger | Today's surface | Notes |
|---|---|---|---|
| `VZErrorInternal` (1) | Catch-all internal error inside Vz. | `RunningVm::stop` returns `Err(ElastosError::Compute(format!("vz stop (vm_id='…'): {description}")))`. The supervisor maps this to `anyhow::anyhow!("Vz VM stop failed for '{name}': {err}")`. **No panic.** | The host-side carrier fd is still open; the supervisor leaves the bridge to drain via natural EOF when the operator retries `stop_capsule` or the supervisor process exits. |
| `VZErrorVirtualMachineGuestPaniced` (3) | Guest kernel panic during the stop sequence (rare but documented). | Same surface as `VZErrorInternal`: typed `ElastosError::Compute`. | Distinguishable in the message string. Future ticket: classify into a dedicated `ElastosError::GuestPanic` variant for richer telemetry. |
| `VZErrorOperationCancelled` (10) | A `stop` issued while the VM is still in the `Starting` state, or a second `stop` issued before the first resolved. | Same surface. | The supervisor's `running` map is already empty at this point (we removed the entry before calling `vm.stop`), so retries are idempotent. |
| `VZErrorInvalidVirtualMachineConfiguration` (8) | Cannot fire on stop (only on `validateWithError:`); listed here so the failure-mode matrix is complete vs. the entire VM lifecycle. | N/A on stop. | Surfaced during `VzVmBuilder::build` instead. |
| Custom errors (domain ≠ `VZErrorDomain`) | Vz private errors not documented in `VZErrorCode`. | Captured as the raw `NSError.description` string. | The `format_validate_error` helper attaches an entitlement hint when the message contains "entitlement"; the stop path does not (entitlement errors don't fire on stop). |

What is NOT in scope for Day 5:

* Direct mapping `VZErrorCode` → typed Rust variants. The
  current string-based surface is sufficient for operator
  diagnostics; a typed enum is Phase 4 Day 7 work or later.
* Resource-leak detection via OS-level introspection. Apple's
  `Virtualization.framework` does not expose per-VM memory
  accounting through public API; `vmmap` / `task_info` are
  out-of-process and require private entitlements. Documented
  as a non-goal.
* Cross-host Carrier message round-trips (Phase 5).
* Apple-runner CI provisioning.

### Test inventory

```
elastos-server/tests/vz_shutdown_semantics.rs
  in_flight_cross_vm_rpc_surfaces_provider_error_when_target_vm_stops
  closing_host_side_of_carrier_socket_terminates_bridge_loop_within_one_second

elastos-server/src/supervisor.rs (lib tests)
  supervisor::tests::prune_stale_mac_artifacts_removes_overlays_and_sockets_but_preserves_unrelated_files
  supervisor::tests::fresh_supervisor_does_not_falsely_report_stale_overlay_files_as_running
```

All tests pass under both `RUST_TEST_THREADS=1` and `=4`. The
new integration tests are gated by `#![cfg(target_os = "macos")]`
so the Linux Cargo target ignores them. The new unit tests are
gated by `#[cfg(target_os = "macos")]` at the test-function
level so the lib-test target on Linux continues to build them
out.

### CI gates (Day 5 acceptance)

* `cargo fmt --all -- --check` — clean.
* `cargo clippy --workspace --all-targets -- -D warnings` —
  clean on Mac.
* `RUST_TEST_THREADS=1 cargo test -p elastos-server` — green.
* `RUST_TEST_THREADS=4 cargo test -p elastos-server` — green.
* `scripts/check-linux-untouched.sh bcf5a0a` — green (Linux
  crates untouched; only `elastos-server` edited and only
  inside `cfg(target_os = "macos")` blocks or platform-agnostic
  structs).

### Forward pointers (Phase 4 Day 6+)

* Defensive timeout inside `VzMachineHandle::stop` so a stuck
  Apple completion handler doesn't wedge the supervisor.
* Typed `VZErrorCode` → `ElastosError` variant mapping for
  richer operator telemetry.
* Optional `Supervisor::new_and_prune` constructor for
  single-instance deployments that *do* want startup cleanup.
* Cross-host Carrier message audit (still Phase 5).
* Apple-runner CI provisioning (out of scope until parity gate
  is fully closed).
