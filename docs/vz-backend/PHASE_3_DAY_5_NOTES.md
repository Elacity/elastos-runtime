## Phase 3 Day 5 — `VZVirtualMachineDelegate`-driven exit codes + vsock host→guest dial primitives

> Outcome log. Status: complete. The supervisor's
> `wait_for_exit` on Mac now reports the *real* exit reason
> (clean shutdown vs Apple-reported crash vs host-initiated
> stop) instead of the Day-3 placeholder `0`-for-everything,
> and the FFI primitive for `VZVirtioSocketDevice.connectToPort:`
> is wired through `RunningVm::connect_vsock` /
> `VzProvider::vsock_connect`. End-to-end host→guest provider
> bridging on Mac is deferred to Day 6 because that integration
> needs a Mac-eligible capsule manifest to validate against.

### Goal (recap)

Day 4 wired the Carrier console socketpair so capsule code
inside the VM can talk to host providers via `/dev/hvc1`. The
two remaining "is this really like Linux?" gaps were:

1. **Exit-code distinction.** `RunningVm::wait_for_exit_code`
   polled `state != Running` every 100 ms and returned `0` for
   every terminal state. The supervisor couldn't tell clean
   shutdown from crash; `elastos status <handle>` showed
   `Stopped` for both.
2. **Host→guest vsock.** Crosvm exposes `AF_VSOCK` directly on
   Linux. On Mac, Apple's only supported channel is
   `VZVirtioSocketDevice.connectToPort:completionHandler:` —
   we had the device attached (Day 2) but no FFI surface to
   actually dial it.

Day 5 closes (1) end-to-end and lands the primitive that
closes (2) — full provider-bridge integration follows in Day 6.

### What landed

1. **`elastos-vz/src/ffi/delegate.rs`** (new). Custom NSObject
   subclass `ElastosVzDelegate` declared via
   `objc2::define_class!` and conforming to
   `VZVirtualMachineDelegate`. Three delegate methods:

   | Apple selector | Reason | Exit code |
   |---|---|---|
   | `guestDidStopVirtualMachine:` | Guest cleanly stopped (`poweroff -h`, `init 0`) | 0 |
   | `virtualMachine:didStopWithError:` | Apple tore the VM down because of an internal error | 1 |
   | `virtualMachine:networkDevice:attachmentWasDisconnectedWithError:` | Logged only; non-terminal | n/a |

   The delegate holds a shared
   `Arc<Mutex<Option<oneshot::Sender<DelegateExit>>>>` (first-
   to-take-it-wins). Subsequent delegate fires are logged but
   do not poison the channel.

2. **`VzMachineHandle::new`** creates the delegate + oneshot
   pair before the VM is constructed, then dispatches
   `setDelegate:` onto the VM's associated queue
   immediately after `initWithConfiguration_queue` returns.
   Apple uses a *weak* reference for delegates, so the
   `Retained<ElastosVzDelegate>` is held inside
   `VzMachineHandle` for the handle's lifetime via a
   `SendableDelegate` newtype (same pattern as `SendableVm`).

3. **`VzMachineHandle::stop`** now also signals the shared
   exit channel with `DelegateExit::HostInitiatedStop` after a
   successful `stopWithCompletionHandler:` completion — Apple
   does NOT fire the delegate on host-initiated stops, so
   without this any `wait_for_exit` racing against a stop
   would hang.

4. **`VzMachineHandle::wait_for_exit`** is a new public
   `pub(crate) async fn` that awaits the receiver and maps
   `DelegateExit → i32`. Consumes the receiver on first call;
   subsequent calls return a typed
   `"receiver already consumed"` error. The supervisor's
   single-waiter contract (one `wait_for_exit` per capsule
   handle) makes this safe.

5. **`RunningVm::wait_for_exit_code`** replaced its
   100 ms-polling `loop` with a single `handle.wait_for_exit()
   .await`. The exit code surfaces through `Result<i32>` exactly
   like before — no supervisor changes were necessary.

6. **`elastos-vz/src/ffi/vsock.rs`** gained `connect_vsock`,
   the FFI primitive for `VZVirtioSocketDevice.connectToPort:`:

   ```text
   VZVirtualMachine.socketDevices[0]
                          │
                          ▼  (downcast to VZVirtioSocketDevice)
                          │
                          ▼  connectToPort:completionHandler:
                          │
                          ▼  on completion:
                          │
                          ▼  VZVirtioSocketConnection.fileDescriptor
                          │
                          ▼  dup(fd) ──► OwnedFd to caller
                          │
                          ▼  Apple's connection drops at block end
                              (closes its own fd; our dup keeps the
                               socket endpoint alive)
   ```

   The `dup` pattern mirrors Day 4's carrier socketpair
   handling — Apple's docs are explicit that
   `VZVirtioSocketConnection.fileDescriptor` is owned by the
   connection object, so we duplicate before letting the
   connection drop.

7. **`RunningVm::connect_vsock(port)`** public method delegates
   to `VzMachineHandle::connect_vsock`. Returns
   `std::os::fd::OwnedFd` so callers (Day 6+) can wrap in
   `tokio::io::unix::AsyncFd` or a `std::fs::File` for
   blocking I/O — same shape `vm_provider.rs::try_vsock_connect`
   already uses for its Linux AF_VSOCK fd.

8. **`VzProvider::vsock_connect(handle, port)`** mirrors the
   other provider lifecycle methods. Returns `CapsuleNotFound`
   for unknown handles or handles that have been moved out via
   `take_running_vm` — the supervisor can dispatch on a single
   error variant.

9. **Tests** (Mac-gated unless noted):

   | Crate | Test | Asserts |
   |---|---|---|
   | `elastos-vz` (lib) | `delegate_exit_maps_to_expected_codes` (new) | `GuestCleanStop`/`HostInitiatedStop → 0`; `StoppedWithError → 1`. |
   | `elastos-vz` (lib) | `delegate_signal_exit_sends_first_terminal_observation_only` (new, `#[tokio::test]`) | First call resolves the receiver; second is a no-op at the channel level; the shared sender is consumed. |
   | `elastos-vz` (lib) | `vsock_connect_result_passes_through_apple_errors_with_op_prefix` (new) | Diagnostic error string includes op label + nil-pointer reason. |
   | `elastos-vz` (integ) | `vz_provider_vsock_connect_fails_closed_for_unknown_handle` (new) | `vsock_connect` on an unknown handle returns `CapsuleNotFound`. |

10. **Docs:** this file; `PLAN.md` Phase 3 header advances to
    "Day 5 complete"; `MAC.md` capability matrix updates the
    exit-code row from "polling, all 0" to "delegate-driven,
    distinguishes clean stop from crash".

### Apple-API notes that shaped Day 5

- **Delegate weak references.** Apple's `VZVirtualMachine.delegate`
  is a `__weak` property — typical Cocoa pattern to avoid
  retain cycles. If we let the
  `Retained<ElastosVzDelegate>` drop while the VM is still
  alive, the VM's weak ref becomes nil and delegate methods
  silently stop firing. `VzMachineHandle::delegate` holds the
  retained delegate to ensure parity with the VM lifetime.
- **`setDelegate:` threading.** Apple requires VM property
  mutations to happen on the VM's associated dispatch queue,
  so the delegate is set via `queue.as_raw().exec_sync(...)`
  after init.
- **Host-initiated stops don't fire the delegate.** Apple's
  semantics distinguish "the VM was told to stop" (completion
  handler on `stopWithCompletionHandler:`) from "the VM
  stopped on its own" (delegate). We bridge the gap by
  signalling `DelegateExit::HostInitiatedStop` on the shared
  channel inside our `stop()` implementation.
- **`VZVirtioSocketConnection` fd ownership.** Apple's docs:
  "The file descriptor is owned by the
  `VZVirtioSocketConnection`. It is automatically closed when
  the object is destroyed." We never call `close` explicitly;
  the `Retained<VZVirtioSocketConnection>` drops when the
  completion block exits, taking its fd with it. Our `dup`'d
  fd keeps the kernel socket endpoint alive on its own.

### What is still *not* working after Day 5

- **Full host→guest provider bridge on Mac is not yet wired.**
  Day 5 ships the FFI primitive (`RunningVm::connect_vsock`)
  but `elastos-server/src/vm_provider.rs::try_vsock_connect`
  still uses the Linux-only `socket(AF_VSOCK)` path. Wiring it
  through requires a `MacVsockDialer` trait threaded into
  `VmCapsuleProvider` plus Mac-side `register_provider_route`
  changes (the Mac arm currently returns `None` because
  `vm_config.network` is `None` — no TAP). Day 6.
- **TAP networking** is still rejected with the typed
  entitlement-required error from Day 2 (no silent NAT
  downgrade).
- **`stop_capsule` race.** If the supervisor calls
  `stop_capsule` *while* `wait_for_exit` is awaiting, the
  shared channel resolves with `HostInitiatedStop` from the
  `stop()` path — but the wait task may have already taken
  the receiver. The current code returns the
  `HostInitiatedStop` exit code from `wait_for_exit` and
  silently ignores the second take from `stop_capsule`'s
  status update. This is fine in practice (both paths agree
  the VM has stopped) but worth a follow-up if any caller
  observes mismatched ordering.

### Linux-untouched evidence

- `scripts/check-linux-untouched.sh bcf5a0a`: green.
- All new code lives in `elastos-vz` (Mac-only crate) or its
  Mac-gated test files. No `elastos-server` changes for the
  delegate work; `elastos-server` still receives the same
  `Result<i32>` shape from `RunningVm::wait_for_exit_code`
  it always has.
- `cargo clippy --workspace --all-targets -- -D warnings`:
  clean on Mac AND Linux. One new `type VsockResultSender`
  alias to suppress `clippy::type_complexity` on the shared
  oneshot type.
- 511 tests green locally on Mac (Day 4 ended at 507; Day 5
  added 4 — three in `elastos-vz` lib, one in the integration
  smoke suite).

### Day 6 handoff

1. **`vm_provider.rs` Mac integration.** Introduce a
   `VsockDialer` trait or `Arc<dyn Fn(u32) -> ...>` closure on
   `VmCapsuleProvider`. On Mac, `start_capsule_vm_macos`
   captures a `Weak<RwLock<HashMap<String, RunningCapsule>>>`
   into a closure that looks up the handle, downcasts to
   `CapsuleBackend::VzVm`, and calls `RunningVm::connect_vsock`.
   Linux's `socket(AF_VSOCK)` path stays unchanged.
2. **First-party capsule provider end-to-end.** Validate the
   full chain (`localhost-provider` capsule on Mac → vsock
   dial from a sibling capsule via the supervisor's provider
   registry) once Day 6 wires the dialer. This is the smoke
   test that proves "Mac capsules can serve each other".
3. **`VZVirtualMachineDelegate` smoke test.** A standalone
   integration test that boots a real (minimal) kernel and
   asserts the delegate exit code matches the kernel's
   shutdown reason. Today's coverage is unit-level (mocking
   `signal_exit` directly); a boot-driven test would land
   alongside the `vm-debug boot` Phase 2 Day 5 harness.
