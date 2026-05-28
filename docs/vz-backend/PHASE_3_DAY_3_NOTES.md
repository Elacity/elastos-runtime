## Phase 3 Day 3 — `CapsuleBackend::VzVm` + supervisor `RunningCapsule` registration

> Outcome log. Status: complete. The
> [Day-1 audit table](PHASE_3_DAY_1_PORT_PLAN.md#audit-table) is
> now fully closed — every AG row, every GAP row, every LX/VZ
> row has either landed or been deferred with a named milestone.

### Goal (recap)

Day 2 ported the full substrate-agnostic launch prefix into the
Mac arm but still exited with a typed
`"supervisor RunningCapsule registration is pending Day 3"`
error after `VzProvider::start` succeeded. That meant:

- `elastos ps` could not list Mac capsules.
- `elastos status <handle>` could not report on them.
- `elastos stop <handle>` could not stop them.

Root cause from the Day-1 audit (row L1197-1210):
`CapsuleBackend::Vm` is hard-wired to
`Box<elastos_crosvm::vm::RunningVm>`. Day 3 adds a sibling
`VzVm` variant, extends every match-on-backend site, and rewires
`start_capsule_vm_macos` to insert into `self.running` instead
of failing closed.

### What landed

1. **`CapsuleBackend::VzVm(Box<elastos_vz::RunningVm>)`** (Mac-gated)
   in `elastos-server/src/supervisor.rs`. Sibling to the
   existing `Vm` and `Carrier` variants. Cfg-gate keeps Linux
   builds free of the Vz type — every `match` arm below has a
   matching `#[cfg(target_os = "macos")]` branch.

2. **Match arms extended** (4 sites):

   | Site | Behaviour |
   |---|---|
   | `reap_dead_capsules` (L635) | Reads `vm.is_running()` on the Vz handle (or cached status if the handle is absent) — stopped Vz VMs get reaped on the same tick crosvm VMs do. |
   | `stop_capsule` (L1654) | Calls `RunningVm::stop().await` (which dispatches `VZVirtualMachine.stopWithCompletionHandler` on the per-machine dispatch queue), then removes the rootfs overlay the supervisor created in Day 2's `build_vm_config_for_mac`. |
   | `wait_for_exit` (L1694) | Calls the new `RunningVm::wait_for_exit_code` (see below), logs exit, removes the overlay. |
   | `capsule_status` (L1745) | Reads `vm.is_running()` to surface `"running"` / `"stopped"` — same shape as the crosvm arm. |

3. **`VzProvider::take_running_vm(&self, handle) -> Result<RunningVm>`**
   (new, `elastos-vz/src/provider.rs`). Removes the
   `RunningVm` from the provider's internal `vms` map and
   returns ownership to the caller. After this call,
   `VzProvider::stop` / `::status` / `::info` for the same
   handle correctly return `CapsuleNotFound` — the supervisor
   owns the lifecycle from that point on. The `VzMachineHandle`
   inside `RunningVm` carries its own `Arc<VzDispatchQueue>`,
   so dropping the provider does **not** stop the VM (we have
   the provider drop right after `take_running_vm` to prove
   this).

4. **`RunningVm::wait_for_exit_code(&mut self) -> Result<i32>`**
   (new, `elastos-vz/src/vm.rs`). Polls the Vz `state` property
   through the dispatch queue at a 100 ms interval and returns
   `0` when the VM leaves `Running`. Approximates the
   supervisor's existing wait semantics — distinguishing clean
   shutdown from crash needs `VZVirtualMachineDelegate`
   notifications, which is Day 4+ work. On non-macOS, it fails
   closed with `PHASE_1_STUB_MESSAGE`.

5. **`Supervisor::start_capsule_vm_macos`** ends with a real
   insertion:

   ```text
   load_with_vm_config → start → take_running_vm → drop(provider)
                                                  ↓
                                  RunningCapsule { backend: VzVm(...) }
                                                  ↓
                                       self.running.write().insert(...)
                                                  ↓
                                              Ok((handle, cid))
   ```

   No more `bail!` after a successful start. `elastos ps`,
   `elastos status`, `elastos stop` work on Mac.

6. **Tests** (Mac-gated unless noted):

   | Crate | Test | Asserts |
   |---|---|---|
   | `elastos-server` | `start_capsule_vm_macos_seam_surfaces_vz_validation_error_after_phase3_day3` (renamed from the Day-1 seam test) | On a kernel-less / rootfs-less host the seam still surfaces a typed `VzProvider::load_with_vm_config` validation error. The Day-2 pending-registration message is gone. |
   | `elastos-server` | `capsule_status_returns_running_capsule_for_vz_vm_variant` (new) | Synthetic `CapsuleBackend::VzVm` flows through `capsule_status` without an exhaustiveness panic; returns the expected fields. |
   | `elastos-server` | `stop_capsule_removes_vz_vm_from_running_map` (new) | `stop_capsule` dispatches through the new arm and removes the entry. |
   | `elastos-server` | `reap_dead_capsules_removes_stopped_vz_vm_entry` (new) | The reaper handles the new variant correctly — stopped Vz VMs are removed on the same background tick. |
   | `elastos-vz` | existing `RunningVm` tests | Unchanged. The new `wait_for_exit_code` is exercised via the supervisor wait dispatcher; a standalone unit test would need either a real Vz boot or a mock for `current_state`, both out of scope for Day 3. |

7. **Docs:** this file + close-out of the final row in
   [`PHASE_3_DAY_1_PORT_PLAN.md`](PHASE_3_DAY_1_PORT_PLAN.md);
   `PLAN.md` Phase 3 header; `MAC.md` capability matrix.

### Apple-API note that shaped Day 3

The dual-ownership question — "does the supervisor own the
`RunningVm`, or does the provider?" — is resolved here in
favour of the supervisor. The Linux flow has the same shape:
`elastos_crosvm::vm::RunningVm` is owned by the supervisor's
`CapsuleBackend::Vm`, not by any provider middleware. The
provider on Mac is now a thin "construct, start, hand off"
seam, mirroring how `elastos_crosvm` is used.

This keeps a **single source of truth** for VM state: only the
supervisor's `running` map needs to be consulted by every
status/stop/wait API. The Vz framework still owns the
underlying `VZVirtualMachine` object (via `VzMachineHandle`),
but the *capsule lifecycle* is the supervisor's responsibility,
end to end.

### What is still *not* working after Day 3

- Capsule code inside the VM cannot talk to the host Carrier
  bridge. The bridge listener exists (Day 2 work) and the
  guest boots with `elastos.carrier_path=/dev/hvc1`, but the
  Vz console attachment in
  `elastos-vz/src/ffi/console.rs::build_carrier_console_slot`
  is still a placeholder — bytes do not yet flow guest↔host on
  `/dev/hvc1`. **Day 4** wires a real `socketpair` / file
  descriptor onto the `VZSerialPortAttachment` and routes one
  end into the bridge listener.
- vsock from host → guest is not yet bridged (Day 5).
- TAP networking (capsule reachable from host LAN) is rejected
  with a typed entitlement error (Phase 3 Day 4+; needs
  `com.apple.vm.networking`).
- `wait_for_exit_code` returns `0` for every terminal state.
  Distinguishing clean shutdown vs crash needs
  `VZVirtualMachineDelegate` notifications (Day 4+).
- The Day-3 success path (insert into `running`) is exercised
  by synthetic `RunningCapsule`s in tests; the
  end-to-end "real kernel + rootfs → boots → registered in
  `elastos ps`" path is proved by `vm-debug boot` (Phase 2
  Day 5) but not yet by `elastos start <name>` because the
  test environments lack the cached artefacts.

### Linux-untouched evidence

- `scripts/check-linux-untouched.sh bcf5a0a`: green.
- All match arms are exhaustive (no wildcard `_` arms added).
- Linux build is byte-identical apart from the new
  `CapsuleBackend::VzVm` variant — which is cfg-gated out on
  non-macOS targets, so the enum's Linux-side compile-time
  layout is unchanged.
- `cargo clippy --workspace --all-targets -- -D warnings`:
  clean on Mac AND Linux.
- 505 tests green locally on Mac (Day 2 ended at 502;
  Day 3 added 3 supervisor tests).

### Day 4 handoff

The next slice (Day-1 audit table row `1124-1133`, the
"Carrier socket attachment" half — Day 2 only set the boot arg
and spawned the host-side listener):

- `elastos-vz/src/ffi/console.rs::build_carrier_console_slot`
  currently returns a placeholder attachment. Replace it with
  a real `VZFileHandleSerialPortAttachment` (or equivalent)
  backed by a `socketpair` — the supervisor's end stays the
  Unix socket the bridge already listens on (Day 2);
  the guest's end becomes `/dev/hvc1`.
- Wire the supervisor's Carrier socket (created at
  `<socket_dir>/<handle>-carrier.sock`) to the host end of
  the socketpair so existing `RequestEnvelope` /
  `ResponseEnvelope` flow works unchanged.
- First-party capsule end-to-end boot becomes possible: `chat`,
  `did-provider`, etc. should be launchable via `elastos start`
  on Mac.

After Day 4, Days 5+ tackle vsock bridging and finer-grained
exit reporting.
