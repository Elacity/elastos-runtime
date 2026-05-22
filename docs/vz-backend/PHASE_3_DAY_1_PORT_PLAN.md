## Phase 3 Day 1 — supervisor → VzProvider port plan

> Audit-only document. Status as of `73cd293` on `sash/local-test`
> (Phase 2 Day 5 complete + the host-portable fixture fix).
>
> Goal: a single, auditable map of the Linux microVM launch path
> in [`elastos-server/src/supervisor.rs`](../../elastos/crates/elastos-server/src/supervisor.rs)
> so subsequent Phase 3 days can clear it one slice at a time
> without re-walking the whole function. Every line reference in
> this document is to `supervisor.rs` unless otherwise noted.

### Why this document exists

The Day 5 audit established that `elastos start <microvm-capsule>`
on macOS still `bail!()`s at L962-974, *before* any of the rich
launch-path setup (VmConfig build, session token injection, boot
args composition, Carrier bridge spawn, rootfs overlay) runs.
Phase 3 Day 1's job is to remove that pre-launch wall — but only
in the way that keeps Linux byte-identical and refuses to fake
parity on Mac.

The plan in [`docs/vz-backend/PLAN.md` §Phase 3](PLAN.md#L253)
sizes the whole supervisor port at **1–2 weeks**. This document
is the line-by-line evidence backing that sizing.

### Method

The Linux launch path is `Supervisor::start_capsule_vm` —
specifically the second half, from the substrate-validation block
(L957) to the `RunningCapsule` insertion (L1210). Every operation
in that range is classified into exactly one of four buckets:

| Bucket | What it means |
|---|---|
| **AG** | Substrate-agnostic. Same code can run on Linux and Mac unchanged. |
| **LX** | Linux/crosvm specific. Must NOT run on Mac. |
| **VZ** | Mac/Vz specific. Must NOT run on Linux. |
| **GAP** | Substrate-agnostic in spirit but currently hard-wired to a Linux type/field. Needs minor refactor (e.g. a thin alias / cfg-gate / new method) so both substrates can share it. |

### Audit table

| Lines | Operation | Bucket | Mac port status |
|---|---|:---:|---|
| 957-960 | `elastos_crosvm::is_supported()` → `/dev/kvm` check | LX | Already cfg-gated to `target_os = "linux"`. No port. |
| 962-974 | macOS `bail!()` with `PHASE_1_STUB_MESSAGE` | VZ | **THIS DAY**: replace with substrate-agnostic walk + `VzProvider::load_with_vm_config + start`. |
| 976-977 | `not(any(linux, macos))` final `bail!()` | AG | No change. |
| 979-987 | `self.crosvm_config.validate()` + `verify_host_artifact("crosvm"/"vmlinux")` | LX | Mac substitute = `VzProvider::new(VzConfig::default())` + provider-side validation. `VzConfig::validate()` already exists in `elastos-vz`. |
| 989-995 | vsock CID allocation (`self.next_cid.write().await`) | AG | Vz does not let the host pick the guest CID (Phase 0 §D pitfall #5). The supervisor's CID is therefore *advisory* on Mac — used only for log lines and the supervisor's own bookkeeping; not handed to Vz. Need to keep allocating it so log lines stay diffable across substrates. |
| 997 | `Self::unique_handle(name, cid)` | AG | No change. |
| 999-1017 | Launch config normalisation (`_elastos_interactive`, `_elastos_capsule_args`) | AG | No change. |
| 1019-1024 | `VmConfig::from_manifest(&manifest, &capsule_dir, &self.crosvm_config.kernel_path)` + `vm_config.vsock_cid = cid` + `boot_args` `+= "elastos.data_dir=/opt/elastos"` + `interactive_stdio` | GAP | Linux uses `elastos_crosvm::VmConfig`; Mac uses `elastos_vz::VmConfig`. Both have `from_manifest`. The types are structurally similar but distinct. Day 1 takes the **cfg-gated parallel path** — Linux constructs the crosvm one, Mac constructs the Vz one. Converging the two types is **explicitly out of Phase 3 Day 1 scope** (it touches `elastos-crosvm` which is Linux-untouched-protected). |
| 1027-1033 | TAP networking (`manifest.permissions.guest_network`) | LX | Mac does not support TAP via Vz without `com.apple.vm.networking` entitlement (`PLAN.md` §Phase 3 L259). Day 1 leaves this as: if the manifest sets `guest_network: true` on Mac, fail closed with a typed error pointing to Phase 3 Day 4+ bridged-network work. |
| 1035-1054 | TERM/winsize boot args (interactive only) | AG | No change. |
| 1056-1093 | Session token injection (shell vs Capsule, session_registry, `vm_config.with_session` or `boot_args += elastos.token=...`) | AG | No change. The crosvm and Vz `VmConfig::with_session` shapes are compatible (both take `&str` token + `&str` api_addr). |
| 1095-1101 | Command payload base64 (`elastos.command=`) | AG | No change. |
| 1103-1113 | Capsule args base64 (`elastos.capsule_args=`) | AG | No change. |
| 1115-1122 | `elastos.provider_port=` boot arg | AG | No change. |
| 1124-1133 | Socket dir + Carrier socket + `vm_config.carrier_socket_path` + `elastos.carrier_path=/dev/hvc0` | GAP | On Mac, the Carrier console lives at `/dev/hvc1` because `/dev/hvc0` is the **kernel console** (`ffi/console.rs::build_kernel_console` + `ffi/builder.rs:142`). Day 1 takes the cfg-gated parallel path: Linux keeps `=hvc0`, Mac uses `=hvc1`. The host-side socket binding to the Vz `carrier_console` slot is **Phase 3 Day 3+ work** (`ffi/builder.rs:56-62` already documents this). Day 1 only sets the kernel arg correctly; the bytes don't actually flow yet. |
| 1135-1144 | Rootfs overlay creation (copy `rootfs.ext4` → `<overlay>/<handle>.ext4`) | AG | No change. Vz attaches the overlay file as a `VZVirtioBlockDevice` via `ffi/block.rs::build_block_device`. Ext4 format on Apple Silicon kernels works (Day 5 boot evidence). |
| 1146 | `provides = manifest.provides.clone()` | AG | No change. |
| 1148-1174 | `carrier_bridge::spawn_carrier_bridge(socket, registry, token, ctx)` | AG | No change. Returns a `JoinHandle`; the guest connects to the Unix socket via virtio-console once `/dev/hvc1` ↔ socket attachment lands (Day 3+). |
| 1177-1180 | `RunningVm::new(vm_config, manifest, socket_path)` + `vm.start(&self.crosvm_config.crosvm_bin)` | LX/VZ | **Different types**: Linux's `RunningVm` = `elastos_crosvm::vm::RunningVm`; Mac's `RunningVm` = `elastos_vz::vm::RunningVm`. The latter is already wired (Day 3) — boots a real Vz machine via `VzMachineHandle`. Day 1 calls `VzProvider::load_with_vm_config + start` instead of constructing `RunningVm` directly. Linux path stays byte-identical. |
| 1182-1185 | Log line "Launched VM '%s' …" | AG | No change. |
| 1187-1194 | `register_provider_route` (requires guest IP from TAP) | GAP | On Mac with NAT-only networking, `vm.config.network` is `None` so the route is `None` — same shape as a Linux capsule without `guest_network: true`. Day 1: no change. |
| 1197-1210 | `running.write().insert(handle, RunningCapsule { backend: CapsuleBackend::Vm(Box<RunningVm>), ... })` | GAP | `CapsuleBackend::Vm` is hard-wired to **crosvm**'s `RunningVm`. On Mac we'd need a `CapsuleBackend::VzVm(Box<elastos_vz::vm::RunningVm>)` variant, OR an enum erasure via a trait. **Day 1 explicitly defers**: the Mac path successfully starts the VM via `VzProvider::start` (proved by Day 5 boot), but does NOT register it in `running`. Surfaces a typed error: `"vz: VM started but supervisor RunningCapsule registration pending (Phase 3 Day 2 — needs CapsuleBackend::VzVm enum variant)"`. This is honest: the VM is observable in `tracing` and `vm_console` output but is invisible to `elastos ps`, `elastos stop`, etc. Day 2 closes this. |

### Stub re-classification

The Day 5 audit listed three `PHASE_1_STUB_MESSAGE` stubs in
`elastos-vz/src/provider.rs` (L232-272). Closer reading reveals
their **API shape is wrong** for the supervisor flow, not just
their implementation.

**Apple's hard constraint**: `VZVirtualMachineConfiguration` is
frozen the instant `VZVirtualMachine::initWithConfiguration:queue:`
is invoked. `VzProvider::load` currently calls that on L138-143,
which means **no boot arg, no session token, no network change
can be applied after `load`**. The stubbed surfaces
(`set_session_for_vm` / `append_boot_args_for_vm` /
`set_network_for_vm`) all assume mid-life-cycle mutation, which
Vz does not permit.

The supervisor's existing Linux flow does NOT call any of these
stubs — it bakes every boot arg into `vm_config.boot_args`
**before** handing the config to `RunningVm::new`. The Mac path
must do the same.

Day 1 therefore:

- Adds `VzProvider::load_with_vm_config(vm_config, manifest)`
  that accepts a fully-baked `VmConfig` (boot args, session
  token, command payload, carrier path — all already applied).
- Refactors the existing `VzProvider::load(path, manifest)` to be
  a thin wrapper over `load_with_vm_config` (DRY).
- Replaces `append_boot_args_for_vm` body with a typed error
  pointing operators at `load_with_vm_config`:
  `"vz: append_boot_args_for_vm is unsupported — VZVirtualMachineConfiguration is frozen after load; bake boot args into VmConfig and call VzProvider::load_with_vm_config(vm_config, manifest) instead. See docs/vz-backend/PHASE_3_DAY_1_PORT_PLAN.md."`
- Leaves `set_session_for_vm` and `set_network_for_vm` for Day 2
  (they have the same wrong-shape issue; the fix is to remove
  them entirely and let the supervisor compose the boot args
  directly — Day 2 work).

### What changes on Day 1 (executable summary)

1. `elastos-vz/src/provider.rs`:
   - **NEW**: `pub async fn load_with_vm_config(&self, vm_config: VmConfig, manifest: CapsuleManifest) -> Result<CapsuleHandle>`. Real, working implementation.
   - **REFACTOR**: `load(path, manifest)` → calls `load_with_vm_config` internally. Same external contract.
   - **REPLACE**: `append_boot_args_for_vm` body → typed error explaining the new pattern. Same fail-closed semantics; better operator UX.
   - **TESTS**: `vz_provider_load_with_vm_config_accepts_baked_boot_args`, `vz_provider_append_boot_args_returns_unsupported_after_phase3_day1`. Existing `vz_provider_session_and_boot_args_still_fail_closed_with_stub` updated to reflect the new (still-fail-closed) `append_boot_args` message.

2. `elastos-server/src/supervisor.rs`:
   - **REPLACE** macOS bail (L962-974) with a `cfg(target_os = "macos")` walk that runs every AG step verbatim, constructs an `elastos_vz::VmConfig` (instead of `elastos_crosvm::VmConfig`), then calls `VzProvider::load_with_vm_config(vm_config, manifest) + start(handle)`.
   - **HONEST FAIL POINT**: after `start` succeeds (VM is booting per Day 5), surface a typed error explaining the supervisor's `CapsuleBackend::Vz` variant is pending Day 2. The VM continues running until process exit (no leak — Day 5 lifecycle handles drop correctly), but the supervisor returns control with a named, expected error. This is **fail closed, then explain** per `PRINCIPLES.md` #11.
   - **TESTS**: `start_capsule_vm_on_macos_reaches_vz_provider_load_with_vm_config_before_failing` (the new typed error name is the assertion target).

3. Docs:
   - `docs/MAC.md` — new row in "What works" matrix.
   - `docs/vz-backend/PLAN.md` — Phase 3 line item marked **in progress (Day 1: supervisor seam + load_with_vm_config shipped)**.

### Out of scope (Day 2+)

- `CapsuleBackend::VzVm` enum variant + `RunningCapsule` insertion on Mac.
- Real socketpair attachment for the Carrier console (`/dev/hvc1` ↔ host Unix socket).
- Real vsock host listener bridging.
- TAP network support (Vz bridged mode, requires Apple entitlement).
- Removing `set_session_for_vm` / `set_network_for_vm` and migrating the supervisor to bake everything into `VmConfig` directly.
- Booting any real first-party capsule end-to-end. The Day 1
  supervisor seam reaches `VzProvider::start` for a synthetic
  capsule fixture in tests; Day 5's `vm-debug boot` already
  proved real Linux guests boot — Day 1 does NOT re-prove that.

### Audit checklist (gates this document signs off on)

- [x] Every operation in `start_capsule_vm` (L957-1213) is in
  exactly one row of the audit table.
- [x] Every "GAP" item names the smallest fix that unblocks it.
- [x] Every "deferred to Phase 3 Day 2+" item names the
  approximate slice it belongs to.
- [x] The boot-args-mutation API mismatch (frozen
  `VZVirtualMachineConfiguration`) is identified, documented, and
  has a named replacement (`load_with_vm_config`).
- [x] Linux behaviour change set: **zero**. Every Mac change is
  inside `cfg(target_os = "macos")` arms.
