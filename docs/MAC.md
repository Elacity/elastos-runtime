# macOS

Honest current status of `elastos-runtime` on macOS, plus the named path
to first-class support.

> This document is the truthful Mac-side companion to
> [`state.md`](../state.md) ("Support boundary"). If anything in this file
> conflicts with the canonical [`PRINCIPLES.md`](../PRINCIPLES.md) or
> [`state.md`](../state.md), those win.

## TL;DR

- The runtime daemon **compiles and runs natively on macOS** today (Apple
  Silicon, `aarch64-apple-darwin`). Developers can build and iterate on
  the daemon code without a Linux VM.
- `WASM` and `data` capsules — for example `home`, `system`, `chat-room`,
  `gba-emulator` — run natively on macOS with the same isolation
  guarantees they have on Linux (wasmtime sandbox + capability tokens).
- `MicroVM` capsules **do not yet run on macOS**. The repo refuses to
  pretend otherwise: `elastos setup --list` will report
  `[skip] <name> — not available for darwin-arm64` for any `type: microvm`
  capsule. This is intentional fail-closed behaviour per
  [`PRINCIPLES.md` #11 *Fail Closed, Then Explain*](../PRINCIPLES.md).
- The named, scoped project that makes macOS a first-class ElastOS host
  with **identical microVM isolation** to Linux is the **Apple
  Virtualization.framework backend** ([`vz-backend/PLAN.md`](vz-backend/PLAN.md)).

## What runs natively on macOS today

| Capsule kind | Substrate | Status on Mac |
|---|---|---|
| `type: wasm` (e.g. `home`, `system`, `chat-room`) | wasmtime, capability tokens | Same as Linux |
| `type: data` (e.g. `documents`, `library`, `inbox`, `gba-emulator`) | static assets, served by gateway | Same as Linux |
| `type: microvm` (e.g. `chat`, `agent`, `localhost-provider`, `did-provider`, `shell`, `webspace-provider`, `ipfs-provider`, `tunnel-provider`, ...) | KVM + crosvm on Linux; Apple Vz on macOS *(in progress — see below)* | **Phase 5 Day 1 begins the Linux-smoke-parity port to Mac.** `scripts/local-carrier-setup-smoke.sh` is now Mac-aware via the new `scripts/lib/cross-platform.sh` helper library (bash-3.2-clean `pid_is_running`, `read_pids_into_array`, `vz_host_is_capable`, `vz_discover_launchable_capsule`). Three new operational lanes: `ELASTOS_VZ_SMOKE_DRY_RUN=1` for CI parse-only fast lane, a Mac pre-flight that detects missing `components.json` darwin-arm64 entries with an actionable Phase-6 pointer, and a Vz substrate readiness probe at the tail. `ELASTOS_VZ_SMOKE_FORCE_PROCEED=1` is the escape hatch for Phase-6 developers iterating on the components.json restoration. 15 helper unit tests in `scripts/lib/cross-platform-test.sh` cover every function on both bash 3.2 (macOS) and bash 4+ (Linux). Linux behaviour byte-identical. See [`vz-backend/PHASE_5_DAY_1_NOTES.md`](vz-backend/PHASE_5_DAY_1_NOTES.md) for the operator runbook + Phase-6 gap analysis; [`vz-backend/PHASE_5_PLAN.md`](vz-backend/PHASE_5_PLAN.md) for the full 8-day Phase-5 plan. **Phase 4 Day 8 closes Phase 4 with a typed `VzErrorReport` readback RPC.** A new `SupervisorRequest::CapsuleVzError { handle }` variant (`op = "capsule_vz_error"`) returns the structured `elastos_vz::VzErrorReport` projection of `RunningVm::last_vz_error()` — `kind_label` (stable telemetry filter, e.g. `"vz_internal"`, `"vz_timed_out"`, `"vz_unknown"`) + `description` are always set; `domain` + `code` populate only for `VzError::Unknown` (so future / unmodelled Apple variants stay greppable without a binding update); `vm_id` + `budget_secs` populate only for `VzError::TimedOut` (so dashboards can size the fleet-wide `VzConfig::stop_timeout`). Every optional field skip-serialises so legacy dashboards keep working unchanged and field presence becomes a typed signal. `SupervisorResponse::vz_error: Option<VzErrorReport>` is the wire-format field; `capsule_status` enriches stopped Vz capsules with BOTH `last_exit_reason` (Day 7) AND `vz_error` (Day 8) for single-query observability. The dispatcher's three-state outcome (`Found(None)` / `Found(Some(report))` / `NotFound`) maps cleanly onto `status: "ok"` + skip-serialised field / `status: "ok"` + populated field / `status: "not_found"`. With Day 8 the Mac substrate has the same observability surface as Linux's crosvm path: structured per-capsule error readback, telemetry labels for every terminal state, and stable JSON wire formats for both the alert path and the triage path. See [`vz-backend/PHASE_4_DAY_8_NOTES.md`](vz-backend/PHASE_4_DAY_8_NOTES.md). **Phase 4 Day 7 makes Vz failures structurally typed.** A new `VzError` enum (`elastos_vz::VzError`) classifies Apple's `VZErrorCode` (`Internal` / `InvalidConfiguration` / `InvalidState` / `InvalidStateTransition` / `NetworkError` / `OperationCancelled` / `NotSupported`) plus our synthetic `TimedOut { vm_id, budget }` for the Day 6 stop-timeout case and a forward-compatible `Unknown { domain, code, description }` for codes the binding doesn't yet model. `VzMachineHandle::start` / `stop` return `Result<(), VzError>` directly; the FFI boundary reads `NSError.domain` + `.code` + `.localizedDescription` and routes the typed variant through. The companion `VzExitReason` enum (`GuestCleanStop` / `HostInitiatedStop` / `StoppedWithError` / `ForcedAfterTimeout`) is the canonical exit-code + telemetry-label source of truth and is cached on `RunningVm::last_exit_reason()`. `SupervisorResponse` gains an optional `last_exit_reason: Option<String>` field (`#[serde(skip_serializing_if = ...)]` — backward compatible with legacy dashboards). `Supervisor::stop_capsule` and `capsule_status` populate it with one of `guest_clean_stop` / `host_initiated_stop` / `stopped_with_error` / `forced_after_timeout` so operators piping `elastos status` JSON into Datadog / Grafana can alert on `forced_after_timeout` rate without grepping log lines. See [`vz-backend/PHASE_4_DAY_7_NOTES.md`](vz-backend/PHASE_4_DAY_7_NOTES.md). **Phase 4 Day 6 hardens Vz stop semantics + bridge teardown observability.** `VzConfig::stop_timeout` (default 30 s, configurable) puts a typed-error upper bound on Apple's `stopWithCompletionHandler:` block — wedged framework calls no longer pin the supervisor's `stop_capsule` indefinitely (Mac has no `kill -9` equivalent on a Vz VM, so this is the only enforcement point). On timeout, `wait_for_exit` waiters resolve with the new `DelegateExit::ForcedAfterTimeout` exit code 137 (matches Linux's `128 + SIGKILL(9)` convention so operator tooling sees a consistent forced-stop marker across substrates). `BridgeContext::on_terminate` exposes the Carrier dispatch loop's lifecycle as an `Arc<tokio::sync::Notify>` — the Mac supervisor awaits this notify with a 10 s budget after `vm.stop()` resolves so it can deterministically log "bridge terminated cleanly" vs "bridge orphaned — continuing best-effort." Best-effort cleanup posture: overlay removal, provider-route unregistration, and bridge wait all run regardless of whether the stop succeeded, so a wedged stop never leaves on-disk state that blocks future launches. See [`vz-backend/PHASE_4_DAY_6_NOTES.md`](vz-backend/PHASE_4_DAY_6_NOTES.md). **Phase 4 Day 5 closes shutdown semantics + crash-recovery audit on Vz.** The full teardown graph (`Supervisor::stop_capsule` → `RunningVm::stop` → `VzMachineHandle::stop` → drop chain → bridge task exit) is documented with surface deltas vs Linux/crosvm in [`vz-backend/PHASE_4_DAY_5_NOTES.md`](vz-backend/PHASE_4_DAY_5_NOTES.md). Two integration tests prove the graceful-failure surface: an in-flight cross-VM RPC against a stalled provider VM that subsequently stops returns a typed `ProviderError` (`unhealthy` / `closed` / `timed out`) within 30 s — never silent, never hung, never panicked; closing the host side of a Carrier socket terminates the dispatch loop and a fresh bridge becomes responsive in <1 s. An opt-in `Supervisor::prune_stale_mac_artifacts` helper (Mac-only, idempotent) removes orphaned overlay + socket files from a prior crashed supervisor without touching unrelated files in the same directories; a fresh `Supervisor` does not falsely report stale on-disk artifacts as running. The Vz teardown failure-mode matrix documents which `VZErrorCode` values can fire on stop (`VZErrorInternal`, `VZErrorVirtualMachineGuestPaniced`, `VZErrorOperationCancelled`) and how each is surfaced today (typed `ElastosError::Compute`, no panic). **Phase 4 Day 4 audits capsule-manifest plumbing end-to-end on the Vz launch path.** Every manifest field the Linux supervisor's `launch_capsule` reads is honoured by `build_vm_config_for_mac` / `start_capsule_vm_macos` at parity (full table in [`vz-backend/PHASE_4_DAY_4_NOTES.md`](vz-backend/PHASE_4_DAY_4_NOTES.md)). The only Mac-specific addition is a pre-flight memory guard: a manifest requesting more RAM than the host can satisfy is rejected with an actionable error before any handle/CID/overlay is allocated, instead of bubbling up Apple's opaque `VZErrorInvalidVirtualMachineConfiguration` at boot. A new visibly-skipping integration test (`elastos-server/tests/vz_supervisor_smoke.rs`) drives the production launch pipeline against a real installed capsule and verifies launch within 30 s + clean stop within 10 s; an optional `provides:` round-trip exercises the Phase 4 Day 3 cross-VM dispatch path end-to-end against a real Vz boot. **Phase 4 Day 3 audits cross-VM provider RPC under N concurrent microVMs.** The dispatch path `run_carrier_bridge_loop → handle_request → ProviderRegistry::send_raw → VmCapsuleProvider::send_raw → VmRawBridge::send_raw_blocking` has one shared-state touch per layer: the registry holds a read lock only across the `Arc<dyn Provider>` clone (released before await); each `VmCapsuleProvider` has its own `Mutex<Option<VmIo>>` that serializes against a single provider VM but never against a sibling VM. The host bridge has NO request-id allocator by design — pairing is by strict order over the per-VM connection, which the Mutex enforces. New tests prove: (a) two synthetic provider VMs + three consumer tasks issuing 60 cross-VM RPCs see every nonce paired with the right provider and the right consumer (no cross-talk, no losses, no deadlocks), (b) 100 parallel `PendingRequestStore` create + half-grant / half-deny calls end with exactly 50 Granted + 50 Denied (no losses, no double-resolution), (c) 1000 parallel `CapabilityManager::validate` calls finish in <5s wall-clock (proves the verify path stays read-mostly under load). See [`vz-backend/PHASE_4_DAY_3_NOTES.md`](vz-backend/PHASE_4_DAY_3_NOTES.md). **Phase 4 Day 2** audited Carrier-bridge multiplexing under N concurrent Vz VMs ([`vz-backend/PHASE_4_DAY_2_NOTES.md`](vz-backend/PHASE_4_DAY_2_NOTES.md)). **Phase 4 Day 1** unlocked N concurrent Vz VMs by moving Apple's GCD serial queue to per-`VZVirtualMachine` ownership and exercising the supervisor's `next_cid` allocator with a 100-parallel-caller uniqueness test (see [`vz-backend/PHASE_4_DAY_1_NOTES.md`](vz-backend/PHASE_4_DAY_1_NOTES.md)). Earlier Phase 3 days landed the supervisor → `VzProvider` seam (Day 1), the launch prefix (Day 2), `CapsuleBackend::VzVm` (Day 3), the Carrier console socketpair (Day 4), delegate-driven exit codes + the host→guest vsock primitive (Day 5), `MacVsockDial` provider-bridge integration (Day 6), and the `com.apple.vm.networking`-entitlement-gated bridged networking path (Day 7 — see [`vz-backend/PHASE_3_DAY_7_NOTES.md`](vz-backend/PHASE_3_DAY_7_NOTES.md)). |

### `guest_network: true` on macOS — binary signing requirements

The runtime check uses `SecTaskCopyValueForEntitlement` against the
current process's embedded entitlements plist. The two reachable
states are:

| Binary | Outcome for a `guest_network: true` capsule |
|---|---|
| **Signed with `com.apple.vm.networking` entitlement** (release build provisioned via the Apple Developer ID + entitlement request) | `VZBridgedNetworkDeviceAttachment` attached to the VM, deterministic MAC from `NetworkConfig.guest_mac`, capsule sees a routable interface bridged to the host's primary network. |
| **Unsigned dev binary** (every `cargo build` artifact, every CI runner) | Builder returns `ElastosError::Compute` naming `com.apple.vm.networking` and `guest_network`. Operator is told to either drop the manifest flag (capsule runs NAT-only) or install the signed dev build. NO silent NAT downgrade — the capsule explicitly asked for routable networking and must either get it or be told why it can't. |

NAT-only capsules (the vast majority — every `home`, `chat`, `agent`, etc.) are not affected by this gate and run identically on signed and unsigned binaries.

The browser-hosted Home surface
(`http://127.0.0.1:8090/apps/home/`) and its child apps (System, Inbox,
Documents, Library, Chat Room, GBA) are reachable from macOS through
`elastos gateway`. This matches [`state.md`](../state.md) L88: *"The
default Home path must remain a KVM-independent browser-hosted adapter
so macOS and Windows stay in scope without pretending to offer Linux
parity."*

## Why MicroVM capsules don't run on Mac yet

ElastOS's central security thesis — *every `type: microvm` capsule in
its own hardware-isolated microVM, communicating only through Carrier
with explicit capability tokens* — is true on Linux via KVM + the
[`elastos-crosvm`](../elastos/crates/elastos-crosvm/) crate. KVM is a
Linux-kernel feature; macOS does not have it.

The runtime preserves that contract by failing closed when the
substrate is missing, rather than silently swapping to a weaker
isolation model. The supervisor explicitly checks:

```rust
// elastos/crates/elastos-server/src/supervisor.rs (L930)
if !elastos_crosvm::is_supported() {
    bail!("/dev/kvm not available — crosvm requires KVM. Cannot launch capsule '{name}'.");
}
```

That is the truthful, principled answer on Mac today: *the substrate is
not available, so the capsule will not run.* Not a downgrade.

> **Note on Slice B (commits `a02045e`, `a229638`, `d623ba7`):** an
> earlier exploratory branch added darwin entries to
> [`components.json`](../components.json) for `shell`,
> `localhost-provider`, `did-provider`, and `webspace-provider`. Those
> entries were misleading — they implied "this `type: microvm` capsule
> runs natively on Mac" while in fact the runtime was launching them as
> plain host-binary subprocesses without microVM isolation. The
> Pre-Work step of [`vz-backend/PLAN.md`](vz-backend/PLAN.md) removes
> those entries so the manifest tells the truth. The daemon-portability
> improvements from those same commits (`pid_is_alive`,
> `ELASTOS_DATA_DIR`, smoke-script Bash 3.2 portability, the
> [`elastos-crosvm`](../elastos/crates/elastos-crosvm/) `cfg`-gated
> non-Linux build) are preserved — they are universally-good
> portability work and do not touch the Linux microVM substrate.

## Path to first-class support

The named, scoped project is the **Apple Virtualization.framework
backend**. Read the full plan in
[`vz-backend/PLAN.md`](vz-backend/PLAN.md). Headlines:

- **Strategy.** Add `elastos-vz` as a *sibling* crate to
  [`elastos-crosvm`](../elastos/crates/elastos-crosvm/), implementing
  the same
  [`ComputeProvider` trait](../elastos/crates/elastos-compute/src/traits.rs)
  on top of Apple's `Virtualization.framework`. No edits to
  [`elastos-crosvm`](../elastos/crates/elastos-crosvm/). No edits to
  the trait. No edits to the capsules. No edits to Carrier. No edits to
  the capability-token plane. Linux execution path byte-equivalent to
  pre-change.
- **End state.** Every `type: microvm` capsule artifact that runs on
  Linux runs on Mac with **identical hardware-enforced isolation,
  identical Carrier semantics, identical capability tokens, identical
  fail-closed behaviour**. The capsule itself does not know it is on
  Mac.
- **Effort.** 6–10 weeks for one focused systems-Rust engineer, phased
  Pre-Work → Phase 0 (1 wk research) → Phase 1 (scaffold) → Phase 2
  (first guest boot) → Phase 3 (virtio plumbing) → Phase 4 (first
  capsule end-to-end) → Phase 5 (hardening) → Phase 6 (ship).

## Honest trust delta

For [`PRINCIPLES.md` #12 *Docs, Code, Tests, and Ops Must Agree*](../PRINCIPLES.md),
the one honest difference between Linux and macOS isolation that the
Vz backend does *not* erase:

|  | Linux today | macOS after Vz backend |
|---|---|---|
| Hardware isolation | CPU virt extensions (VT-x / SVM / ARM EL2) | CPU virt extensions (Apple Silicon EL2) |
| Hypervisor | [`elastos-crosvm`](../elastos/crates/elastos-crosvm/) — open source, auditable, vendored | Apple `Virtualization.framework` — closed source, signed and shipped with macOS |
| Trust source | We audit crosvm | We trust Apple's signed binary |
| Integrity verification | Implicit (distro packaging) | Apple System Integrity Protection + binary code signing |

**Net:** hardware-level isolation parity is full. The trust *source*
differs. This is the same trade-off Docker Desktop, OrbStack, Lima,
Tart, and every Mac-targeting VMM accepts. We disclose the delta
honestly; we do not paper over it.

The capability-token plane, the Carrier transport, the capsule
artifact, and the fail-closed semantics are **unchanged** by this
trade-off.

## What this means for users today

- **You can develop the daemon code on a Mac.** `cargo build` and
  `cargo test` work. The daemon process runs, gateway serves the
  browser Home, WASM and data capsules execute.
- **You cannot run microVM capsules on a Mac.** Until the Vz backend
  lands, install of any `type: microvm` capsule will be skipped with a
  clear "not available for darwin-arm64" message. To run the full
  microVM-isolated ElastOS today, use Linux (`x86_64-linux` or
  `aarch64-linux`).
- **If you need to test microVM capsules from a Mac**, run the runtime
  inside a Linux VM (Lima, Apple Vz-managed manually, or a remote
  Linux host) and connect to it. This mirrors how Docker Desktop and
  the Kubernetes control plane handle the Mac case.

## First boot on Apple Silicon (Phase 2 Day 5)

As of Phase 2 Day 5 you can boot a real Linux guest end-to-end.
The control plane (codesign → load → validate → start → console
forwarder → state polling) is fully wired; the missing piece
from Day 4 — "a real arm64 kernel + initramfs that Vz accepts"
— is now a one-shot `fetch-vz-kernel.sh` invocation against
Ubuntu's archived cloud images. The Day 5 outcome log
([`vz-backend/PHASE_2_DAY_5_NOTES.md`](vz-backend/PHASE_2_DAY_5_NOTES.md))
records the verbatim guest console output.

Operator recipe:

```bash
# 1. Build the runtime binary.
cargo build -p elastos-server

# 2. Sign it with com.apple.security.virtualization so Apple's
#    VZVirtualMachineConfiguration.validateWithError accepts the
#    config. Re-run after every `cargo build`.
scripts/dev/sign-elastos-vz/sign.sh

# 3. Fetch a Vz-compatible kernel + initramfs + rootfs.
#    Downloads Ubuntu 22.04 arm64 cloud-image artifacts to
#    ~/.local/share/elastos/vz-bin/, verifies their SHA-256
#    against checksums baked into the script, gunzips the
#    kernel to a raw Linux Image, and converts the qcow2
#    disk to raw via `qemu-img` (install via `brew install qemu`
#    if missing). Idempotent.
scripts/dev/fetch-vz-kernel.sh

# 4. Boot the guest. Guest kernel printk streams via the
#    `vm_console` tracing target — visible by default at
#    `info` level. Ubuntu's rootfs lives on /dev/vda1, not
#    the whole disk, so we override the default --boot-args.
target/debug/elastos vm-debug boot \
  --rootfs    ~/.local/share/elastos/vz-bin/rootfs.img \
  --kernel    ~/.local/share/elastos/vz-bin/Image \
  --initramfs ~/.local/share/elastos/vz-bin/initramfs.img \
  --memory-mb 1024 \
  --boot-args 'console=hvc0 root=/dev/vda1 rw'

# Press Ctrl-C to stop. The VM also stops itself if the
# guest reaches an end state (panic, shutdown).
```

You can swap in your own kernel + rootfs at any time — the
fetch script is a known-working starting point, not a hard
dependency. Anything that satisfies Vz's contract works
(arm64 raw Linux Image, raw disk image, optional initramfs
mmap-able by Vz).

### Common error shapes

If you skip the codesign step (#2 above) you'll see this
error when `vm-debug boot` calls `provider.load`:

> `vz validate (vm_id='…'): missing com.apple.security.virtualization entitlement — sign the binary with scripts/dev/sign-elastos-vz/ (Phase 2 Day 4) or see docs/MAC.md. Apple error: …`

If the kernel artifact isn't a Vz-compatible arm64 Image,
Apple returns the same opaque message from `provider.start`:

> `Internal Virtualization error. The virtual machine failed to start.`

That's the signal that the kernel format is wrong — either
it's a bzImage (x86), a compressed vmlinuz that wasn't
decompressed, or a kernel built without arm64 Image format
support. Re-run `fetch-vz-kernel.sh` to get a known-good
artifact, or check that your own kernel's first 0x44 bytes
contain the `ARMd…PE\0\0` magic at offset 0x38/0x40.

If you pass `--initramfs <path>` but the file is missing,
validation errors before any Vz call:

> `boot loader: initramfs file does not exist at /path/to/initramfs`

## Cross-references

- The plan: [`docs/vz-backend/PLAN.md`](vz-backend/PLAN.md)
- Day 4 outcome log: [`docs/vz-backend/PHASE_2_DAY_4_NOTES.md`](vz-backend/PHASE_2_DAY_4_NOTES.md)
- Day 5 outcome log: [`docs/vz-backend/PHASE_2_DAY_5_NOTES.md`](vz-backend/PHASE_2_DAY_5_NOTES.md)
- Codesign helper: [`scripts/dev/sign-elastos-vz/README.md`](../scripts/dev/sign-elastos-vz/README.md)
- Kernel fetcher: [`scripts/dev/fetch-vz-kernel.sh`](../scripts/dev/fetch-vz-kernel.sh)
- The principles this plan obeys: [`PRINCIPLES.md`](../PRINCIPLES.md)
  (#10, #11, #12 in particular)
- The runtime's support boundary: [`state.md`](../state.md)
  ("Support boundary" section)
- The convergence context with PC2/Home:
  [`docs/PC2_CONVERGENCE.md`](PC2_CONVERGENCE.md)
- The Linux microVM substrate (untouched by this work):
  [`elastos/crates/elastos-crosvm/`](../elastos/crates/elastos-crosvm/)
