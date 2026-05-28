## Phase 3 Day 2 — substrate-agnostic prefix port + stub retirement

> Outcome log. Status: complete. Day 3 picks up
> `CapsuleBackend::VzVm` and supervisor `RunningCapsule`
> registration (the last AG row in the
> [Day-1 audit table](PHASE_3_DAY_1_PORT_PLAN.md#audit-table)).

### Goal (recap)

Day 1 shipped the bare supervisor → `VzProvider` seam:
`elastos start <microvm>` on macOS reached
`VzProvider::load_with_vm_config` instead of `bail!()`, but with
a **minimal** `VmConfig` — no session token, no command payload,
no capsule args, no Carrier bridge, no rootfs overlay. The Day-1
audit (`PHASE_3_DAY_1_PORT_PLAN.md`) classified each missing
operation as **AG** (substrate-agnostic), meaning the Linux code
could be ported into the Mac arm with no semantic change.

Day 2 ports every AG row.

### What landed

1. **`Supervisor::build_vm_config_for_mac`** (new, Mac-gated,
   `elastos-server/src/supervisor.rs`). Factored helper that
   replicates the substrate-agnostic prefix of
   `launch_capsule` L997-1144 against `elastos_vz::VmConfig`:

   - vsock CID alloc + handle (advisory CID on Mac per Phase 0 §D #5).
   - `_elastos_interactive` / `_elastos_capsule_args` extraction +
     launch_config sanitisation.
   - `VzVmConfig::from_manifest` + `vsock_cid` + `elastos.data_dir=/opt/elastos`.
   - Provider-wide initramfs default if `vm_config.initramfs_path` is `None`.
   - Interactive TERM/winsize injection (`TIOCGWINSZ` + `TERM` env;
     both POSIX, work on Darwin).
   - Session token injection — shell capsule gets `self.shell_token`,
     all others get a fresh `SessionType::Capsule` token from
     `self.session_registry`. Mac always follows the no-TAP branch:
     `boot_args += " elastos.token=<t>"` only, no `elastos.api=`
     (the capsule uses the microVM Carrier bridge, not HTTP).
   - Command payload base64 (`elastos.command=`).
   - Capsule args base64 (`elastos.capsule_args=`).
   - `elastos.provider_port=<VM_PROVIDER_PORT>` when the manifest
     advertises `provides`.
   - Carrier socket path under `self.crosvm_config.socket_dir`
     (OS-agnostic location reused from the Linux path).
   - `vm_config.boot_args += " elastos.carrier_path=/dev/hvc1"`
     — Mac-specific delta from the Linux flow (`/dev/hvc0` is the
     Vz kernel console).
   - Rootfs overlay under `self.crosvm_config.rootfs_cache_dir/overlays/`
     with bytes copied verbatim from `<capsule_dir>/rootfs.ext4`,
     rewiring `vm_config.rootfs_path` to the overlay.

2. **`Supervisor::start_capsule_vm_macos`** signature change:
   gained `config: serde_json::Value` so it receives the same
   launch payload the Linux arm does. Body now:

   - Fails closed before any VM work if
     `manifest.permissions.guest_network == true`, with a typed
     message naming `com.apple.vm.networking` and Phase 3 Day 4+.
     No silent NAT downgrade.
   - Builds the full `VmConfig` via `build_vm_config_for_mac`.
   - Spawns the microVM Carrier bridge on the returned socket
     path (same call shape as Linux L1148-1174). The listener
     exists; the guest does not yet receive bytes through it
     because the Vz console attachment is still a placeholder
     (Phase 3 Day 4+).
   - Calls `VzProvider::load_with_vm_config + start`.
   - Returns the Day-3-pending typed error after a clean
     `provider` drop (VM stopped, no leak).

3. **`VzProvider::set_session_for_vm`** + **`VzProvider::set_network_for_vm`**
   (`elastos-vz/src/provider.rs`) retired. They previously
   carried `PHASE_1_STUB_MESSAGE`; they now return a typed,
   named migration error in the same style as
   `append_boot_args_for_vm` did on Day 1. The bodies explicitly
   point operators at `VmConfig::with_session` /
   `load_with_vm_config` and at the entitlement requirement for
   bridged-mode networking.

4. **Tests:**

   | Crate | New / updated test | Purpose |
   |---|---|---|
   | `elastos-server` | `start_capsule_vm_macos_reaches_vz_provider_after_phase3_day1` (existing) | Confirms the seam is still reached after the Day-2 changes. |
   | `elastos-server` | `build_vm_config_for_mac_bakes_full_phase3_day2_prefix` (new) | Asserts every Day-2 boot arg (`data_dir`, `command`, `capsule_args`, `carrier_path=/dev/hvc1`) is baked; carrier-socket path embeds the handle; the wrong hvc0 path is never used. |
   | `elastos-server` | `start_capsule_vm_macos_fails_closed_when_manifest_requests_tap_network` (new) | Asserts TAP is rejected with a typed message naming the entitlement and the capsule name; no silent downgrade. |
   | `elastos-server` | `build_vm_config_for_mac_creates_rootfs_overlay_when_source_present` (new) | Asserts the overlay file is created under `rootfs_cache_dir/overlays/`, bytes round-trip, source rootfs untouched. |
   | `elastos-vz` | `vz_provider_set_session_for_vm_returns_typed_migration_error_after_phase3_day2` (new) | Confirms the retired stub no longer carries `PHASE_1_STUB_MESSAGE` and points at the correct new API. |
   | `elastos-vz` | `vz_provider_set_network_for_vm_returns_typed_migration_error_after_phase3_day2` (new) | Same shape — confirms the entitlement requirement is named for bridged-mode. |
   | `elastos-vz` | `vz_provider_session_and_network_still_fail_closed_with_stub` (retired) | Replaced by the two tests above; the old assertion (`contains(PHASE_1_STUB_MESSAGE)`) is now the *opposite* of correct behaviour. |

5. **Docs:** this file + the status header on
   [`PHASE_3_DAY_1_PORT_PLAN.md`](PHASE_3_DAY_1_PORT_PLAN.md);
   `PLAN.md` Phase 3 line; `MAC.md` capability matrix.

### Apple-API constraint that shaped Day 2

`VZVirtualMachineConfiguration` is frozen the instant
`VZVirtualMachine::initWithConfiguration:queue:` is invoked. Any
"mutate after load" API shape (`append_boot_args_for_vm`,
`set_session_for_vm`, `set_network_for_vm`) is unsupportable on
Vz. The Phase-3-Day-1 plan already named this; Day 2 finishes
the cleanup by retiring the last two stubs and rerouting all
boot-arg composition through `VmConfig` **before**
`load_with_vm_config` runs.

This is also why TAP rejection runs **before** any VM work:
once `VZVirtualMachine` is initialised, no networking change is
possible. Failing early keeps the error close to the cause.

### What is still *not* working after Day 2

- `elastos ps` does not list the Mac capsule. The VM boots and
  then stops cleanly because the supervisor's `running` map
  cannot hold a Vz handle yet — `CapsuleBackend::Vm` wraps
  `Box<elastos_crosvm::vm::RunningVm>`. Day 3 adds a `VzVm`
  variant and the matching insertion.
- `elastos stop <handle>` therefore cannot stop a Mac capsule
  either (Day 3).
- The Carrier bridge listener exists on `<socket_dir>/<handle>-carrier.sock`,
  but the Vz console attachment in `ffi/console.rs::build_carrier_console_slot`
  is still a placeholder (does not yet forward bytes between
  the Unix socket and the guest's `/dev/hvc1`). Day 4 wires the
  real socketpair.
- vsock from host → guest does not work yet. Day 5.
- TAP networking (guest reachable from host LAN) is rejected
  with a typed error. Phase 3 Day 4+ work — needs Apple
  entitlement.
- Booting any real first-party capsule end-to-end. Day 5 of
  Phase 2 already proved a synthetic Linux guest boots; Day 2
  does not re-prove that, and the placeholder Carrier
  attachment means a real capsule launched via
  `elastos start <name>` would boot but not be reachable.

### Linux-untouched evidence

- `scripts/check-linux-untouched.sh bcf5a0a`: green.
- The Linux `start_capsule_vm` flow (`launch_capsule` L957-1213)
  is byte-identical to the pre-Day-1 commit. The macOS arm
  early-returns through `start_capsule_vm_macos`; the Linux arm
  is compiled but unreachable on Mac.
- All `elastos-crosvm` tests pass on Linux CI without
  modification.

### Day 3 handoff

The next slice ([Day-1 audit table row L1197-1210](PHASE_3_DAY_1_PORT_PLAN.md#audit-table)):

- Add `CapsuleBackend::VzVm(Box<elastos_vz::vm::RunningVm>)` to
  the supervisor's `CapsuleBackend` enum (or an erasure trait).
- In `start_capsule_vm_macos`, replace the Day-2 fail-closed
  exit with a real `RunningCapsule` insertion into `self.running`
  — same map the Linux arm uses, same `handle` key.
- Wire `CapsuleStatus`, `WaitCapsule`, `StopCapsule` to dispatch
  through the new variant.
- `elastos ps` / `elastos stop` start working on Mac for the
  first time.

After Day 3, the supervisor seam is complete. Days 4-5 then
attach real bytes to the Carrier console + vsock paths.
