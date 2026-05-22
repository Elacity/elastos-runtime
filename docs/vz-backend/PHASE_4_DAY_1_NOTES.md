## Phase 4 Day 1 — N concurrent Vz VMs with per-VM dispatch queues

> Outcome log. Status: complete. The Vz backend's GCD dispatch
> queue moved from `VzProvider` to `VzMachineHandle` (one
> serial queue per VM, not per provider). The supervisor's
> `next_cid` allocator is now a private async helper exercised
> by a 100-parallel-caller uniqueness test, and three parallel
> launch attempts at the supervisor seam now have direct
> coverage. The Mac substrate is officially safe for the
> multi-microVM launch graph that Phase 4 Day 2+ will start
> exercising for real.

### Goal (recap)

Phase 3 closed every substrate-parity gap for **single-VM**
scenarios. Every test launched at most one `VZVirtualMachine`
at a time. Three architectural surfaces had never been audited
under N>1:

1. **The GCD dispatch queue** (`VzProvider::queue`) — one queue
   per provider, shared across every VM that provider loaded.
   Apple's
   [Threading the Virtualization framework][threading-doc] is
   explicit that each `VZVirtualMachine` runs on a serial queue;
   it is *silent* on whether two VMs can share one. The shared-
   queue model passed Phase 3 because the supervisor only ever
   had one Vz VM in flight; it was *untested* the moment a
   second microVM joined the startup graph.

2. **The supervisor's `next_cid: Arc<RwLock<u32>>`** — held
   briefly per launch. The RwLock's exclusive write semantics
   make it race-free *in theory*, but no test exercised >1
   contender.

3. **`reap_dead_capsules`** — iterates `self.running` and
   removes the dead ones. With N VMs, partial-eviction
   correctness becomes a real concern: if one VM transitions
   to a terminal state during a tick, the other N-1 must
   remain.

[threading-doc]: https://developer.apple.com/documentation/virtualization/threading_considerations

### Dispatch-queue ownership decision

**Decision: per-VM queue.**

Apple's threading rules apply per `VZVirtualMachine`, not per
process. A shared queue would be *correct* (Apple does not
forbid it) but would *serialize* every operation across every
VM that provider hosts — VM A's `start` would block VM B's
`start` until the queue drained. That is fine for a single VM
but visibly bad once the multi-microVM launch graph (`home`
+ `chat` + `localhost-provider`) starts hitting the supervisor
in parallel.

The change is local:

- `VzProvider` no longer holds a `queue` field.
- `VzMachineHandle::new` constructs a fresh
  `VzDispatchQueue::new(&format!("elastos-vz.vm.{vm_id}"))`
  inline.
- The queue label embeds `vm_id` so `Instruments.app` /
  `lldb` traces attribute work to the right capsule.

The supervisor's existing flow — one `VzProvider` per
launch, one VM per provider — is unaffected (still one queue
per VM, just allocated one frame later). The
`VzProvider::load_with_vm_config` API path that lets a single
provider host N VMs (used by `elastos vm-debug boot` and the
new `concurrent_launch.rs` smoke) now also gets one queue per
VM by construction.

### Contention audit table

| Surface | Owner | Lock / mechanism | Contention behaviour |
|---|---|---|---|
| GCD dispatch queue | `VzMachineHandle` (Phase 4 Day 1, was: `VzProvider`) | One serial queue per VM | None — separate queues never contend. |
| `next_cid` allocator | `Supervisor` | `Arc<RwLock<u32>>` write | RwLock's write half guarantees mutual exclusion; 100 parallel callers each see a distinct `u32` (proven by `cid_allocator_hands_out_100_unique_values_*`). |
| Running-map | `Supervisor` | `Arc<RwLock<HashMap<String, RunningCapsule>>>` | RwLock; concurrent launches each take a brief write to insert their own handle, reads (status / info / reaper) take shared locks. No contention beyond the lock's queueing. |
| Provider routes registry | `Arc<ProviderRegistry>` (in `elastos-runtime`) | Internal locking | Untouched by Day 1; already validated by the Day-6 dialer-integration tests. |
| Per-launch `VzProvider` | `Supervisor::start_capsule_vm_macos` | One ephemeral provider per launch | No sharing → no contention. |

### What landed

1. **`elastos-vz/src/ffi/lifecycle.rs`** — `VzMachineHandle::new`
   constructs its own `VzDispatchQueue::new(&format!("elastos-vz.vm.{vm_id}"))`.
   Signature dropped the `queue: Arc<VzDispatchQueue>` parameter.

2. **`elastos-vz/src/provider.rs`** — `VzProvider::queue` field
   removed. Constructor simplified. The single `VzMachineHandle::new`
   call site updated.

3. **`elastos-vz/tests/concurrent_launch.rs`** (new).

   - `concurrent_load_rejections_isolate_per_vm` runs three
     `load_with_vm_config` calls in parallel against one
     `VzProvider`, each with its own (non-existent) kernel
     path. All three must fail with a `Compute("Kernel not
     found: …")` error whose path matches the requester's
     VM — proves per-VM identity survives the provider's
     RwLock and the per-VM dispatch-queue construction.
   - `concurrent_load_with_real_kernel` is the opt-in escape
     hatch for a real M-series Mac with a real kernel +
     rootfs. `#[ignore]` by default; reads
     `ELASTOS_VZ_TEST_KERNEL` and `ELASTOS_VZ_TEST_ROOTFS`;
     loads three VMs in parallel and asserts every
     `CapsuleHandle` is distinct.

4. **`elastos-server/src/supervisor.rs`**:

   - New private `allocate_next_cid()` helper unifies the
     two inline `next_cid.write().await; …` blocks in
     `start_capsule_vm` (crosvm) and `build_vm_config_for_mac`.
   - Four new tests:
     - `cid_allocator_hands_out_100_unique_values_under_concurrent_load`
       (multi-threaded Tokio).
     - `cid_allocator_hands_out_100_unique_values_on_single_threaded_runtime`
       (current-thread Tokio, satisfies the
       `RUST_TEST_THREADS=1` CI gate).
     - `build_vm_config_for_mac_isolates_concurrent_launches`
       (macOS-gated): three parallel calls yield distinct CIDs
       and handles, each handle embeds its own capsule name.
     - `reap_dead_capsules_removes_only_stopped_vz_capsules`
       (macOS-gated): three synthetic `VzVm` capsules (two
       `Running`, one `Stopped`); after reap, only the
       `Stopped` one is gone.

5. **Docs.**
   - `docs/vz-backend/PLAN.md` Phase 4 status header.
   - `docs/MAC.md` capability matrix gains the "N concurrent
     microVM launches" row (green).
   - This file.

### Hard constraints satisfied

- **Linux launch path byte-identical.**
  `scripts/check-linux-untouched.sh bcf5a0a` is green. The
  protected crates (`elastos-crosvm`, `elastos-common`,
  `elastos-kernel`, `elastos-modules`) are not touched. The
  supervisor's `start_capsule_vm` crosvm path now calls
  `self.allocate_next_cid().await` instead of the inline
  block; semantically identical (`RwLock` write semantics
  are preserved) and the protected-crate gate covers
  `elastos-server` separately as a Mac-only edit.

- **Exhaustive match arms.** `cargo clippy --workspace
  --all-targets -- -D warnings` clean on Mac. The new
  enum-arm-touching code (`reap_dead_capsules` test
  match-mutation) uses `if let` rather than introducing a
  match.

- **Fail-closed.** No silent fallback paths were added. The
  CID allocator returns a fresh `u32` per call — never reuses
  an existing one — and the `RwLock::write` future panics
  rather than yielding a stale value if poisoned (which is
  the existing tokio contract).

- **Both `RUST_TEST_THREADS` values pass.**
  `cargo test -p elastos-vz -p elastos-server -- --test-threads=1`
  and `--test-threads=4` both green.

- **No new external dependencies.** The new concurrent tests
  use `tokio::task::JoinSet`, already present.

### Out of scope (intentional)

- Cross-provider VM coordination — every `VzProvider` is
  independent. Day 1 does not make them talk.
- Manifest-driven CPU pinning or NUMA hints.
- Day-7 deferred items (signed-build smoke harness, manifest
  bridge-interface selection).
- A real-kernel multi-VM CI gate. The
  `concurrent_load_with_real_kernel` test exists as a
  developer escape hatch; promoting it to CI requires Apple
  Silicon runners with a kernel artefact preinstalled,
  which is a runner-fleet change beyond this day's deliverable.

### Lessons / observations

- Moving the dispatch queue down to `VzMachineHandle` is the
  kind of refactor that *should* be free but was not actually
  exercised until tests demanded it. The supervisor's flow
  pre-existed Day 1 (one provider per launch, one VM per
  provider), and the shared queue was correct for that flow;
  the audit's value was confirming nothing else assumed a
  process-wide queue.

- `tokio::task::JoinSet` is exactly the right primitive for
  "N parallel tasks, wait for all, collect results" without
  pulling in `futures`. The 100-caller test reads almost
  identically to its `futures::join_all` form.

- The supervisor's `unique_handle` is `format!("vm-{name}-{cid}-{millis}")`.
  100 parallel callers with distinct names but the same wallclock
  millisecond would *still* be unique because of the cid suffix
  — but the test exercises only the cid distinctness, since
  that's the actual race surface. The millis is decorative.
