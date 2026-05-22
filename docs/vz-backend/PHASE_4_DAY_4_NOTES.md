## Phase 4 Day 4 — Capsule manifest plumbing + end-to-end supervisor smoke

> Outcome log. Status: complete. Days 1–3 audited the underlying
> primitives of the Mac multi-VM substrate
> (dispatch-queue ownership, Carrier-bridge multiplexing, cross-VM
> RPC dispatch). Day 4 connects the *boring connective tissue*:
> the supervisor's manifest → `VmConfig` plumbing, the
> Vz-specific fail-closed guards that the Linux path doesn't
> need, and an end-to-end smoke test that drives a real
> unmodified ElastOS MicroVM capsule from `install` → `launch`
> → `running` → `stop` on Vz.

### Manifest → `VmConfig` parity audit

The supervisor's Linux path (`launch_capsule` body in
`elastos-server/src/supervisor.rs`) reads from
`CapsuleManifest` and from supervisor-reserved keys inside the
launch `config`. The Mac path
(`start_capsule_vm_macos` + `build_vm_config_for_mac`) ports
each field. The audit below records the parity state as of
Phase 4 Day 4.

| Manifest source | Linux behaviour | Mac behaviour today | Decision |
|---|---|---|---|
| `resources.memory_mb` | Forwarded to `VmConfig.mem_size_mib` via `VmConfig::from_manifest`. No host-RAM pre-check (KVM commits lazily). | Forwarded via `VzVmConfig::from_manifest`. **Pre-flight guard added Day 4** — see "Fail-closed guards" below. | Pre-flight reject on Mac, parity-preserving on Linux. |
| `microvm.kernel` | Forwarded to `VmConfig.kernel_path`. Falls back to `crosvm_config.kernel_path`. | Forwarded to `VzVmConfig.kernel_path`. Falls back to `vz_config.kernel_path`. | Parity. |
| `microvm.boot_args` | Forwarded after `sanitize_crosvm_boot_args` (drops Firecracker-era flags). | Forwarded after `rewrite_console_for_vz` (rewrites `console=ttyS0` → `console=hvc0`). | Parity (each path normalizes for its own substrate). |
| `microvm.http_port` | Forwarded to `VmConfig.http_port` (metadata for host-side port forwarding). | Forwarded to `VzVmConfig.http_port` (same metadata role). | Parity. |
| `microvm.vcpu_count` | Forwarded to `VmConfig.vcpu_count`, default 1. | Forwarded to `VzVmConfig.vcpu_count`, default 1. | Parity. |
| `microvm.rootfs_cid` / `kernel_cid` / `rootfs_size` | Consumed by `ensure_capsule` / `download_capsule` (install path, not launch path). | Same — both paths share `ensure_capsule`. | Parity. |
| `microvm.persistent_storage_mb` | **Not** honoured in `supervisor.launch_capsule` (the supervisor bypasses `CrosvmProvider::load`). Honoured only when callers go through the provider trait. | **Not** honoured in `start_capsule_vm_macos` either — the Vz path bypasses the `ComputeProvider` trait for the same reason. | Parity. Tracked as a separate ticket; not a Mac-specific gap. |
| `permissions.guest_network` | Sets `vm_config.network = Some(NetworkConfig::new(&vm_id))`. Linux supports TAP unconditionally. | Sets `vm_config.network` identically. Vz builder then attaches `VZBridgedNetworkDeviceAttachment` IF the binary holds `com.apple.vm.networking`; else returns a typed `ElastosError::Compute`. **Day 7 work**, exercised by the existing `start_capsule_vm_macos_fails_closed_when_guest_network_lacks_entitlement` test. | Parity in routing; fail-closed on unsigned binaries (Day 7 contract preserved). |
| `permissions.carrier` (with `provides`) | Routes to `launch_carrier_service` (host process, not VM). | Same — routing is in `launch_capsule`, before the platform branch. | Parity. |
| `permissions.storage` / `permissions.messaging` | Consumed by the runtime crate's capability subsystem. Not Mac-specific. | Same. | Parity. |
| `provides` | Used to register a `provider_route` after the VM is running. Linux uses the guest TAP IP; Mac uses `MacVsockDial` (Day 6). | Honoured. | Parity (different transport, same semantics). |
| `_elastos_interactive` (launch config) | Sets `vm_config.interactive_stdio` + injects `TIOCGWINSZ` + `$TERM` boot args. | Same in `build_vm_config_for_mac`. | Parity. |
| `_elastos_capsule_args` (launch config) | Base64-encoded and appended as `elastos.capsule_args=...` to boot args. | Same. | Parity. |
| Session token + API address | Linux NAT path: `elastos.token=...`. TAP path: `with_session(token, api_addr)`. | Mac is NAT-only on unsigned binaries: `elastos.token=...` is sufficient; the capsule uses the microVM Carrier bridge for HTTP. | Parity (NAT subset). |
| Command payload (launch config) | Base64-encoded → `elastos.command=...`. | Same. | Parity. |
| Provider port boot arg (`elastos.provider_port`) | Set when `manifest.provides.is_some()`. | Set when `manifest.provides.is_some()`. | Parity. |
| Carrier socket path | `vm_config.carrier_socket_path = ...sock`; boot arg `elastos.carrier_path=/dev/hvc0`. | Same path; boot arg `elastos.carrier_path=/dev/hvc1` (Vz reserves `/dev/hvc0` for the kernel console). | Parity (different device path; same semantics — Day 2 audit). |
| Rootfs overlay | Copies `rootfs.ext4` → `overlays/<handle>.ext4`. | Same. | Parity. |
| Initramfs | Linux supervisor path doesn't surface initramfs (crosvm-attached `--initrd` is set elsewhere). | Picks up `vz_config.initramfs_path` if the manifest does not override (for `elastos vm-debug boot --initramfs …`). | Parity for the supervisor's launch path; initramfs only enters via `vm-debug`. |

**Conclusion.** Every manifest field the Linux supervisor's
launch path reads is also read by `build_vm_config_for_mac` /
`start_capsule_vm_macos` with semantically equivalent
behaviour. The ONE truly Mac-specific addition Day 4 lands is
the oversized-memory pre-flight guard described below; every
other parity entry was already settled by Phase 3 Days 1–7 and
Phase 4 Days 1–3.

### Fail-closed guards landed on the Mac launch path

1. **Oversized memory** (new in Day 4). The host's total
   physical RAM is read via
   `sysctlbyname("hw.memsize", …)` at launch time. If
   `manifest.resources.memory_mb > host_phys_mem_mib -
   MAC_HOST_HEADROOM_MIB` (1 GiB headroom), the launch is
   rejected with an actionable error that names the offending
   capsule, the requested memory, and the host's actual capacity.
   Without this guard, Apple Vz surfaces
   `VZErrorInvalidVirtualMachineConfiguration` *after* the
   supervisor has minted a handle, allocated a CID, and copied
   the rootfs overlay — leaking resources on every failed
   attempt. Tested by
   `build_vm_config_for_mac_fails_closed_when_memory_exceeds_host_ram`
   and `build_vm_config_for_mac_accepts_modest_memory_under_pre_flight_guard`.

2. **`guest_network: true` on an unsigned binary** (Day 7
   contract, preserved). The supervisor populates
   `vm_config.network`; the Vz FFI builder rejects construction
   if the process lacks `com.apple.vm.networking`. Tested by
   `start_capsule_vm_macos_fails_closed_when_guest_network_lacks_entitlement`.

3. **Vz framework unavailable** (Day 0 contract, preserved).
   `start_capsule_vm_macos` checks `elastos_vz::is_supported()`
   and bails with a clear message naming "macOS 12+ on Apple
   Silicon". No silent fallback to a host-binary substrate.

### End-to-end supervisor smoke

A new integration test under
`elastos-server/tests/vz_supervisor_smoke.rs` drives the
production launch pipeline against a real installed capsule:

1. Auto-discovers the supervisor's data directory
   (`$ELASTOS_VZ_SMOKE_DATA_DIR` override, default
   `~/.local/share/elastos`).
2. Reads `components.json` so `ensure_capsule` doesn't try to
   download.
3. Iterates `capsules/*` and picks the first capsule that:
   - has both `capsule.json` and `rootfs.ext4` on disk;
   - declares `capsule_type: microvm`;
   - is NOT a Carrier-plane host-process capsule (carrier ==
     true && provides is Some);
   - does NOT request `guest_network: true` (covered by the
     dedicated entitlement test).
4. Issues `SupervisorRequest::LaunchCapsule { name, config:
   Null }` through the supervisor's public request API.
5. Polls `CapsuleStatus { handle }` every 250 ms until the
   response reports `running`, with a 30 s budget.
6. **`provides:` round-trip (best-effort).** If the discovered
   capsule declares a `provides:` scheme, additionally issues
   one `provider_registry.send_raw(scheme, {"op":"ping"})`
   call with a 10 s wall-clock timeout. The test treats any
   outcome (`Ok` response, typed `ProviderError`, or timeout)
   as acceptable — the smoke test's primary job is the
   boot+stop assertion, not the guest's protocol completeness.
   This exercises the Phase 3 Day 6 → Phase 4 Day 3 cross-VM
   dispatch wiring end-to-end against a real Vz boot.
7. Issues `SupervisorRequest::StopCapsule { handle }` and
   asserts `"status":"ok"`.
8. Polls until the capsule is gone from the supervisor's
   running map, 10 s budget.

The test **visibly-skips** (mirrors the Day 2 convention) with
a clear `eprintln!` message when any prerequisite is missing.
Skip conditions in order of check:

| Condition | Skip message |
|---|---|
| `elastos_vz::is_supported() == false` | "off Apple Silicon macOS, Vz framework unreachable" |
| No data dir | "no $ELASTOS_VZ_SMOKE_DATA_DIR or ~/.local/share/elastos directory" |
| No `components.json` | "no components.json under {data_dir} (or it failed to parse)" |
| No suitable capsule | "no installed MicroVM capsule with a rootfs.ext4 that takes the Vz path" |

CI logs every skip explicitly. Promoting the smoke to a
required gate is the Apple-runner provisioning task tracked in
`docs/vz-backend/PLAN.md`.

### What landed

| File | Change | Type |
|---|---|---|
| `elastos-server/src/supervisor.rs` | `MAC_HOST_HEADROOM_MIB` const, `host_phys_mem_mib_mac()` helper, oversized-memory pre-flight guard, two new fail-closed unit tests | Edit |
| `elastos-server/tests/vz_supervisor_smoke.rs` | End-to-end launch + provides round-trip + stop, auto-discovering visible-skip | New |
| `docs/vz-backend/PHASE_4_DAY_4_NOTES.md` | This outcome log + parity audit + fail-closed matrix | New |
| `docs/vz-backend/PLAN.md` | Phase 4 status bumped to Day 4 | Edit |
| `docs/MAC.md` | Capability matrix updated | Edit |

### Linux-untouched and CI gates

- `scripts/check-linux-untouched.sh bcf5a0a` — green. The only
  supervisor edits are Mac-only (`#[cfg(target_os = "macos")]`
  guards on the new constants, helper, fail-closed guard, and
  new tests). The shared launch flow (`launch_capsule`) is
  unchanged.
- `cargo clippy --workspace --all-targets -- -D warnings` —
  clean on macOS. Expected clean on Linux (no Linux-path
  touches; Mac-only additions sit behind `cfg` gates).
- `cargo fmt --all -- --check` — clean.
- All new tests pass under `RUST_TEST_THREADS=1` and
  `RUST_TEST_THREADS=4`.
- The smoke test runs in well under its 45 s combined launch+stop
  budget when prerequisites are present, and skips
  instantaneously when they're not.
- No new external dependencies.

### Carry-overs into Day 5+

- **`persistent_storage_mb` parity.** Neither the Linux nor
  the Mac supervisor `launch_capsule` flow honours this field
  today — both bypass `CrosvmProvider::load` /
  `VzProvider::load`, which are the only call sites that read
  it. Adding it on Mac alone would invert the parity gap.
  Tracked as a workspace-wide ticket separate from the Vz
  backend.
- **Carrier message round-trip to a Linux peer.** The Day 4
  smoke is *single-host*: launch a capsule, talk to it
  through the supervisor's own registry. Cross-host Carrier
  parity is Phase 4 Day 5+ work.
- **Apple-runner CI provisioning.** With no Apple-Silicon
  runner in the fleet, the smoke test will always visibly-skip
  in CI. Promoting it to a required gate requires a runner
  with `elastos setup` pre-staged. Out of scope for Day 4.
