# Phase 0 — Scope Confirmation

> Deliverable of Phase 0 in [`PLAN.md`](PLAN.md). Desk-research only;
> no production code. Adheres to the **Linux-untouched gate** and to
> [`PRINCIPLES.md` #11 *Fail Closed, Then Explain*](../../PRINCIPLES.md)
> and [`#12 *Docs, Code, Tests, and Ops Must Agree*`](../../PRINCIPLES.md).
> Every factual claim about Apple APIs, crate versions, and external
> projects is cited.

## Gate-question answers (one-line summaries)

| # | Question | Answer |
|---|---|---|
| A | FFI strategy? | **`objc2-virtualization` 0.3.2** (madsmtm); fallback **`arcbox-vz` 0.1.6** |
| B | Vz feature coverage for our device set? | **Full coverage** for every crosvm device class the runtime uses |
| C | Does the shipped `vmlinux` boot on Vz unmodified? | **Linux-amd64 artifact: unknown until tried; Linux-arm64: no artifact ships today (host-copy strategy)** — Phase 2 prototypes with a known-good arm64 Ubuntu kernel, Phase 6 ships a content-addressed arm64 vmlinux |
| D | Non-obvious Tart + Lima pitfalls? | **10 captured below** (disk cache mode, identifier persistence, console=hvc0, vsock has no CID API, etc.) |
| E | Risk register delta? | **Three new rows + one severity reclassify** — see `Risk register update` |
| F | Honest unknowns? | **Three flagged** below — boot-arg console string, initrd compression, vmlinux build pipeline |
| G | Go / no-go? | **Go.** Proceed to Phase 1 (scaffold). No deal-breaker surfaced. |

---

## A. FFI strategy

### A.1 Candidate survey

| Crate | Version | Last release | Coverage of needed Vz classes | Async model | License | Maintenance signal | Verdict |
|---|---|---|---|---|---|---|---|
| [`objc2-virtualization`](https://crates.io/crates/objc2-virtualization) | 0.3.2 | active | **All 17 classes** the runtime needs (see §B) | Raw bindings; wrap in our own Tokio adapter | `Zlib OR Apache-2.0 OR MIT` | 5 340 downloads/90 d; part of the canonical `objc2` ecosystem maintained by madsmtm; same release cadence as `objc2` itself | **Primary** |
| [`arcbox-vz`](https://crates.io/crates/arcbox-vz) | 0.1.6 | March 2026 | claimed comprehensive; built atop `objc2` | Tokio-first, `async fn`s | (verify in Phase 1) | 572 downloads/90 d; very new | **Fallback** if Phase 1 wrapping overhead turns out high |
| [`virtualization-rs`](https://github.com/suzusuzu/virtualization-rs) (suzusuzu) | — | April 2023 | unclear; partial | — | unclear | 81 GitHub stars, no updates in 12+ months | **Rejected** — abandoned |
| Hand-written `objc2` + `objc2-foundation` bridge | — | n/a | full control | manual | matches host | requires writing everything by hand | **Rejected** unless `objc2-virtualization` regresses |

Sources: [`crates.io/crates/objc2-virtualization`](https://crates.io/crates/objc2-virtualization), [`docs.rs/objc2-virtualization`](https://docs.rs/objc2-virtualization), [`crates.io/crates/arcbox-vz`](https://crates.io/crates/arcbox-vz), [`github.com/suzusuzu/virtualization-rs`](https://github.com/suzusuzu/virtualization-rs).

### A.2 Decision and rationale

**Pick: `objc2-virtualization 0.3.2`.**

1. **Coverage is total** — every class the elastos-vz backend needs is exposed (§B). The crate's struct list at [`docs.rs/objc2-virtualization`](https://docs.rs/objc2-virtualization) includes `VZLinuxBootLoader`, `VZVirtioBlockDeviceConfiguration`, `VZVirtioSocketDeviceConfiguration`, `VZVirtioSocketConnection`, `VZVirtioSocketListener`, `VZVirtioConsoleDeviceConfiguration`, `VZVirtioConsoleDeviceSerialPortConfiguration`, `VZVirtioConsolePortConfiguration`, `VZNATNetworkDeviceAttachment`, `VZBridgedNetworkDeviceAttachment`, `VZFileSerialPortAttachment`, `VZFileHandleSerialPortAttachment`, `VZGenericMachineIdentifier`, `VZGenericPlatformConfiguration`, `VZVirtioEntropyDeviceConfiguration`, `VZVirtioTraditionalMemoryBalloonDeviceConfiguration`, `VZVirtualMachine`, `VZVirtualMachineConfiguration`, and the delegates `VZVirtualMachineDelegate`, `VZVirtioSocketListenerDelegate`, `VZVirtioConsoleDeviceDelegate`.
2. **License fits** — `Zlib OR Apache-2.0 OR MIT` is trivially compatible with elastos-runtime's own license posture.
3. **Trust profile is right** — madsmtm is the canonical author of the Rust↔Objective-C ecosystem (`objc2`, `objc2-foundation`, all framework crates). This is the same crate Apple-API users in the Rust ecosystem standardise on. It is a *thin* binding — closer to "FFI declarations" than "framework" — which means the `elastos-vz` crate keeps full control over async semantics, error handling, lifecycle, and the `ComputeProvider` mapping.
4. **Async control stays with us** — we wrap the raw Vz delegate callbacks into Tokio channels exactly the way [`elastos-crosvm/src/vm.rs`](../../elastos/crates/elastos-crosvm/src/vm.rs) wraps subprocess output today. Consistent with the runtime's existing concurrency model.

**Fallback: `arcbox-vz 0.1.6`** if Phase 1 finds the manual delegate wrapping unmaintainable. Crate is very young (March 2026, 572 downloads/90 days), tokio-first, builds atop `objc2`. Defer the bet until we have first-hand experience with the lower-level binding.

**Rejected by design:** writing a Swift shim with a C ABI. Adds a build-system axis (Swift toolchain, lipo, framework linkage) for no API surface gain over `objc2-virtualization`. Only revisit if the Rust ecosystem regresses badly.

### A.3 Build-system implications

- **Dependency line** for `elastos/crates/elastos-vz/Cargo.toml` (Phase 1):
  ```toml
  [target.'cfg(target_os = "macos")'.dependencies]
  objc2 = "0.6"           # version pinned to whatever objc2-virtualization tracks
  objc2-virtualization = "0.3"
  objc2-foundation = "0.3"
  block2 = "0.6"          # for Objective-C blocks used as completion handlers
  dispatch2 = "0.3"       # for Grand Central Dispatch queues (needed by VZ delegates)
  ```
- **No new system dependencies** — `objc2-virtualization` is pure Rust source linking against the system `Virtualization.framework`. Build needs Xcode command-line tools (already required for any Mac Rust dev).
- **Linker hint** — the crate auto-emits `-framework Virtualization` via Apple's pkg-config equivalent; no manual `build.rs` magic in `elastos-vz`.

---

## B. Vz feature-coverage table

For every device class `elastos-crosvm` uses (from [`elastos-crosvm/src/config.rs`](../../elastos/crates/elastos-crosvm/src/config.rs) and [`vm.rs`](../../elastos/crates/elastos-crosvm/src/vm.rs)), here is the Vz equivalent, the macOS minimum, and any known gap.

| Crosvm primitive | Vz class | Min macOS | Notes / Gap |
|---|---|---|---|
| Linux kernel boot (positional kernel arg + `-p` cmdline) | [`VZLinuxBootLoader`](https://developer.apple.com/documentation/virtualization/vzlinuxbootloader) | 11.0 | Requires direct-boot kernel (uncompressed Image on ARM64). Optional `initialRamdiskURL`. **Must be paired with `VZGenericPlatformConfiguration`**, not the macOS one. ([apple docs](https://developer.apple.com/documentation/virtualization/vzlinuxbootloader)) |
| `--cpus N` / `--mem MiB` | [`VZVirtualMachineConfiguration.cpuCount` / `.memorySize`](https://developer.apple.com/documentation/virtualization/vzvirtualmachineconfiguration) | 11.0 | 1:1 mapping. Memory is in bytes; multiply by `1024*1024`. |
| `--block path=,root=true` for rootfs | [`VZVirtioBlockDeviceConfiguration`](https://developer.apple.com/documentation/virtualization/vzvirtioblockdeviceconfiguration) backed by [`VZDiskImageStorageDeviceAttachment`](https://developer.apple.com/documentation/virtualization/vzdiskimagestoragedeviceattachment) | 11.0 | **Set caching mode to `Cached` not `Automatic`** on Apple Silicon — Lima L48–53 cites UTM #4840 corruption. Set sync mode `Fsync`. ([Lima `vm_darwin.go` L495](https://github.com/lima-vm/lima/blob/8de0d4a2/pkg/driver/vz/vm_darwin.go#L495)) |
| `--block path=` for data disk | Same as above, second instance | 11.0 | Order in the storage devices array determines `vd?` device letters in guest. |
| `--serial type=stdout,hardware=serial,num=1` (16550 UART for kernel console) | [`VZVirtioConsoleDeviceSerialPortConfiguration`](https://developer.apple.com/documentation/virtualization/vzvirtioconsoledeviceserialportconfiguration) backed by [`VZFileSerialPortAttachment`](https://developer.apple.com/documentation/virtualization/vzfileserialportattachment) or [`VZFileHandleSerialPortAttachment`](https://developer.apple.com/documentation/virtualization/vzfilehandleserialportattachment) | 11.0 | **Delta: Vz only exposes serial-via-virtio-console.** Guest console appears at `/dev/hvc0`, not `ttyS0`. Affects boot args: see §C and pitfall #3. |
| `--serial type=unix-stream,hardware=virtio-console,num=1` (Carrier bridge) | [`VZVirtioConsoleDeviceConfiguration`](https://developer.apple.com/documentation/virtualization/vzvirtioconsoledeviceconfiguration) + [`VZVirtioConsolePortConfiguration`](https://developer.apple.com/documentation/virtualization/vzvirtioconsoleportconfiguration) (multi-port array) backed by `VZFileHandleSerialPortAttachment` over a socketpair | 12.0 (multi-port) | Console multiport requires macOS 12+. The runtime today gives the Carrier bridge `/dev/hvc0`; on Mac it will be `/dev/hvc1` (port index after the kernel-console multi-port). Tiny boot-arg adjustment, no protocol change. |
| TAP via `--net tap-name=,mac=` | [`VZVirtioNetworkDeviceConfiguration`](https://developer.apple.com/documentation/virtualization/vzvirtionetworkdeviceconfiguration) + [`VZNATNetworkDeviceAttachment`](https://developer.apple.com/documentation/virtualization/vznatnetworkdeviceattachment) | 11.0 | **NAT is the default we ship.** No entitlement required. Apple-side NAT bridges guest to `vmnet` interface (10.x.x.x/24). For bridged mode use [`VZBridgedNetworkDeviceAttachment`](https://developer.apple.com/documentation/virtualization/vzbridgednetworkdeviceattachment) — requires `com.apple.vm.networking` entitlement, deferred. |
| MAC address | [`VZMACAddress`](https://developer.apple.com/documentation/virtualization/vzmacaddress) | 11.0 | Constructed from `objc2_foundation::NSString` of a colon-form MAC. Stable per-capsule MAC mirrors the crosvm `network.guest_mac` field. |
| Vsock with explicit CID | [`VZVirtioSocketDeviceConfiguration`](https://developer.apple.com/documentation/virtualization/vzvirtiosocketdeviceconfiguration) + [`VZVirtioSocketDevice`](https://developer.apple.com/documentation/virtualization/vzvirtiosocketdevice) (per-VM, no public CID API) | 11.0 | **Delta: no public CID API.** Host→guest: `socketDevice.connect(toPort:completionHandler:)` returns `VZVirtioSocketConnection` exposing a `fileDescriptor`. Guest→host: register a `VZVirtioSocketListener` on a port, delegate receives connections. The `vsock_cid` field in `VmConfig` becomes informational on Mac. Affects `elastos-server/src/vm_provider.rs` (see §D pitfall #5). |
| Per-VM identifier | [`VZGenericMachineIdentifier`](https://developer.apple.com/documentation/virtualization/vzgenericmachineidentifier) | 12.0 | Persist `dataRepresentation()` per capsule to survive runtime restarts. (Lima `getMachineIdentifier` L729–747.) |
| Platform config (Intel/ARM Linux) | [`VZGenericPlatformConfiguration`](https://developer.apple.com/documentation/virtualization/vzgenericplatformconfiguration) | 12.0 | Used for non-macOS guests. Carries the machine identifier. |
| Hardware RNG | [`VZVirtioEntropyDeviceConfiguration`](https://developer.apple.com/documentation/virtualization/vzvirtioentropydeviceconfiguration) | 11.0 | Always attach. Complement to the `random.trust_cpu=on` boot arg the runtime already sets. ([config.rs L252–256](../../elastos/crates/elastos-crosvm/src/config.rs)) |
| Memory balloon | [`VZVirtioTraditionalMemoryBalloonDeviceConfiguration`](https://developer.apple.com/documentation/virtualization/vzvirtiotraditionalmemoryballoondeviceconfiguration) | 12.0 | Attach unconditionally. Phase 5 may use it for runtime-driven memory pressure. Crosvm side does not use balloon today; net-new on Vz only, no behaviour change on Linux. |
| `--pivot-root <dir>` (crosvm sandbox) | — | n/a | **Vz handles sandboxing in-framework.** The Vz process inherits the host's permissions; there is no analogue to crosvm's pivot-root. The `pivot_root_dir` field in `VmConfig` is ignored on Mac. Document. |
| Sigterm / `crosvm stop` | [`VZVirtualMachine.requestStop`](https://developer.apple.com/documentation/virtualization/vzvirtualmachine/requeststop) / [`.stop`](https://developer.apple.com/documentation/virtualization/vzvirtualmachine/stop) | 11.0 / 12.0 | Graceful then forceful. Mirror `RunningVm::stop` in [`elastos-crosvm/src/vm.rs:186–254`](../../elastos/crates/elastos-crosvm/src/vm.rs). |

**Net:** there is **no required device the runtime uses that Vz fails to expose.** The two genuine deltas — virtio-console-only kernel console and CID-less vsock host API — are accommodated by changing **two boot-args strings** and **one host-side socket wrapper**, both `cfg(target_os = "macos")` gated. No Linux behaviour changes.

---

## C. Kernel-config audit

### C.1 What the runtime ships today

From [`components.json`](../../components.json):

```
external.vmlinux = {
  description: "ElastOS guest kernel (6.1.59, CONFIG_VIRTIO_CONSOLE=y) for crosvm capsule boot",
  linux-amd64: { cid: Qmeb1qaqfMiri7G123FWmMz6qt74xhPjAgAJEy8ZSrFKh7,
                 release_path: ... },
  linux-arm64: { strategy: "local-copy", source: "/boot/Image",
                 note: "Upstream crosvm aarch64 test kernel is broken on
                        GICv3-only hosts (Jetson). Uses host kernel instead." }
}
```

Plus the runtime's own kernel-validation contract from [`elastos-crosvm/src/config.rs:107–113`](../../elastos/crates/elastos-crosvm/src/config.rs):

```rust
let has_ext4 = contains_ascii(&bytes, b"ext4");
let has_virtio_blk = contains_ascii(&bytes, b"virtio_blk");
let has_virtio_pci = contains_ascii(&bytes, b"virtio_pci");
```

And the magic-byte format check at [`config.rs:158–166`](../../elastos/crates/elastos-crosvm/src/config.rs):

```rust
fn looks_like_arm64_image(bytes: &[u8]) -> bool {
    bytes.len() > 0x44
        && &bytes[0x38..0x3c] == b"ARMd"
        && &bytes[0x40..0x44] == b"PE\0\0"
}
```

So today the contract is **bzImage on x86_64, raw `Image` on aarch64, with `ext4`/`virtio_blk`/`virtio_pci` string markers present.**

### C.2 What Vz requires (claims vs verification)

Vz on Apple Silicon `VZLinuxBootLoader`:
- Direct-boot kernel image — for ARM64 this is the same **raw `Image`** format the crosvm aarch64 path uses (Lima also uses `vz.NewLinuxBootLoader(kernel, opt...)` on a raw kernel — [Lima L765–787](https://github.com/lima-vm/lima/blob/8de0d4a2/pkg/driver/vz/vm_darwin.go#L765)).
- For x86_64 macOS hosts (Intel Mac path is **out of scope for Phase 6** — deferred), uncompressed `vmlinux` is required, not the bzImage marker the runtime ships. **Therefore we do not even attempt Intel Mac in Phase 6**; deferring closes this question.

Guest kernel `CONFIG_*` needed for the device set in §B:

| Config | Required by | Likely present in 6.1.59 ElastOS amd64 kernel? | Verify how |
|---|---|---|---|
| `CONFIG_VIRTIO_PCI=y` | virtio-blk, virtio-net | **Yes** — validated by `validate_guest_kernel` | strings check exists |
| `CONFIG_VIRTIO_BLK=y` | rootfs, data disk | **Yes** — validated by `validate_guest_kernel` | strings check exists |
| `CONFIG_VIRTIO_CONSOLE=y` (and multiport) | kernel-console-on-virtio + Carrier bridge | **Yes for single-port** (declared in description); **multiport TBD** | Phase 2 boot test |
| `CONFIG_VIRTIO_NET=y` | NAT network | likely yes (linked into Image fmt by builder) | Phase 2 boot test |
| `CONFIG_VIRTIO_VSOCKETS=y` (a.k.a. `CONFIG_VSOCKETS`+`CONFIG_VIRTIO_VSOCKETS`) | host↔guest vsock | **Unverified**; required by the crosvm path too because the runtime opens `AF_VSOCK` from [`vm_provider.rs:177`](../../elastos/crates/elastos-server/src/vm_provider.rs) | Phase 2 boot test — `lsmod \| grep vsock` |
| `CONFIG_VIRTIO_BALLOON=y` | memory balloon | not used today by crosvm path; likely =m or absent | Phase 5; not blocking |
| `CONFIG_HW_RANDOM_VIRTIO=y` | entropy device | runtime sets `random.trust_cpu=on` instead | Phase 2; not blocking |

### C.3 Audit conclusion

For **Phase 2 prototyping (first boot on Vz)** — start with a **known-good Ubuntu cloud arm64 kernel** (e.g. `ubuntu-focal-aarch64`) which is documented to work with Vz across the Lima, Tart, Apple containerization, and `arcbox-vz` examples. This eliminates "kernel config" as a Phase 2 failure mode and isolates Phase 2 to "did my Rust-side Vz wiring work."

For **Phase 6 shipping** — the runtime needs a content-addressed `vmlinux-darwin-arm64` artifact in [`components.json`](../../components.json) under `external.vmlinux.platforms.darwin-arm64`. **Decision deferred to Phase 6.** Three viable options:

1. Build the same 6.1.59 source tree the existing `linux-amd64` artifact came from, with `CONFIG_VIRTIO_VSOCKETS=y` and Vz-multiport friendly virtio-console — **most aligned with the runtime's content-addressed identity model**.
2. Ship a stable upstream LTS arm64 kernel (Ubuntu cloud, signed by Canonical) and pin its checksum — **lower build burden, slightly weaker provenance story**.
3. Embed kernel inside the Vz binary at compile time — **rejected**, breaks the components.json contract that capsule artifacts are content-addressed and updateable independently of the runtime.

**No deal-breaker.** Phase 2 boots with a known-good kernel; Phase 6 picks the shipping artifact. Outcome captured in the `Honest unknowns` section.

---

## D. Tart + Lima study — 10 non-obvious pitfalls

Reading [Lima `pkg/driver/vz/vm_darwin.go`](https://github.com/lima-vm/lima/blob/8de0d4a2/pkg/driver/vz/vm_darwin.go), the Lima PR #1147 ([github.com/lima-vm/lima/pull/1147](https://github.com/lima-vm/lima/pull/1147)) for Vz support, and the `Code-Hex/vz` Go binding ([github.com/Code-Hex/vz](https://github.com/Code-Hex/vz)) that Lima depends on. Tart is Swift-only with no readable Vz internals (single-file `VM.swift` is mostly orchestration; the equivalent Vz-config code lives in `Sources/tart/VM/*.swift` private classes).

### Pitfall 1 — Disk image caching mode

Default `Automatic` on Apple Silicon causes guest disk corruption. Set explicitly to `Cached`. Lima cites UTM #4840 ([utmapp/UTM#4840 comment](https://github.com/utmapp/UTM/issues/4840#issuecomment-1824340975)).

> **`elastos-vz` action (Phase 2):** when wiring `VZDiskImageStorageDeviceAttachment`, pass `cachingMode: .cached` and `synchronizationMode: .fsync` (Lima `vm_darwin.go` L495).

### Pitfall 2 — `VZGenericMachineIdentifier` must be persisted

If the same VM gets a fresh identifier each launch, the guest's kernel sees a new "machine" and stable identifiers in the guest (DHCP lease MAC, systemd machine-id keyed off Vz identifier) drift. Lima writes `machineIdentifier.dataRepresentation()` to a file per VM ([Lima L729–747](https://github.com/lima-vm/lima/blob/8de0d4a2/pkg/driver/vz/vm_darwin.go#L729)).

> **`elastos-vz` action (Phase 2):** persist the identifier under `~/.local/share/elastos/vz/<capsule-id>/identifier` and reload on launch.

### Pitfall 3 — `console=ttyS0` boot arg does not exist on Vz

The runtime currently sets `console=ttyS0` from [`elastos-crosvm/src/config.rs:152–156`](../../elastos/crates/elastos-crosvm/src/config.rs) (crosvm uses a 16550 UART). Vz exposes the kernel console only as `virtio-console` → guest device `/dev/hvc0`. If you boot a Linux guest with `console=ttyS0` on Vz, you get a **silent boot** (kernel logs go nowhere, no oops, just hangs at login).

> **`elastos-vz` action (Phase 2):** override the kernel cmdline on Mac to `console=hvc0` before passing it to `VZLinuxBootLoader.commandLine`. This is a one-line Rust change in `elastos-vz`'s `VmConfig→VZ` translator; the underlying `VmConfig` struct on Linux is untouched and `crosvm` keeps using `console=ttyS0`.

### Pitfall 4 — Multi-port virtio-console requires macOS 12+ and a different config class

`VZVirtioConsoleDeviceSerialPortConfiguration` is **single-port** (used for the kernel console). For the Carrier bridge (a *second* virtio-console port at guest's `/dev/hvc1`), you need `VZVirtioConsoleDeviceConfiguration` + a `VZVirtioConsolePortConfigurationArray` of `VZVirtioConsolePortConfiguration` entries — macOS 12.0+ (`objc2-virtualization` exposes both classes).

> **`elastos-vz` action (Phase 3):** wire the Carrier bridge through the multi-port API. In the guest, `ELASTOS_CARRIER_PATH=/dev/hvc1` instead of `/dev/hvc0` (the Linux convention). This is a boot-arg / env-var change on Mac; the guest's [`elastos-guest::RuntimeClient`](../../elastos/crates/elastos-guest) reads `ELASTOS_CARRIER_PATH` so no guest code change.

### Pitfall 5 — Vz vsock has no public CID API

The runtime's [`elastos-server/src/vm_provider.rs:161–223`](../../elastos/crates/elastos-server/src/vm_provider.rs) opens an `AF_VSOCK` socket and calls `connect(cid, port)`. **`AF_VSOCK` is Linux-only.** On Mac there is no per-VM CID number; the host code calls `VZVirtioSocketDevice.connect(toPort:completionHandler:)` on the **specific VM's** socket device and receives a `VZVirtioSocketConnection` exposing a kernel file descriptor (still a usable socket-like fd via `dispatch_io_*`).

> **`elastos-vz` action (Phase 3):** introduce a `VsockTransport` trait inside `elastos-vz` that exposes the same `(reader, writer, raw_fd)` triple `vm_provider.rs` constructs today. On Mac it wraps the `VZVirtioSocketConnection`'s file descriptor; the `VmConfig.vsock_cid` field becomes informational on Mac.
>
> Then add a small `cfg(target_os = "macos")` arm in `elastos-server/src/vm_provider.rs::try_connect_once` (lines 81–85) that looks up the running VM by handle and asks the Vz provider to open a connection, *instead of* synthesising an `AF_VSOCK` socket. This is an edit to `elastos-server/src/vm_provider.rs` — **outside** the Linux-untouched protected list ([`elastos-crosvm/`, `elastos-runtime/`, `elastos-common/`, `elastos-compute/`](../../docs/vz-backend/PLAN.md) §"Linux untouched"). The Linux arm at L177 stays byte-identical.

### Pitfall 6 — Initrd compression: LZ4 ok, zstd may fail

Apple's `VZLinuxBootLoader` mapping of `initialRamdiskURL` into guest memory has been observed to choke on zstd-compressed initrds; LZ4 (Ubuntu cloud images) and gzip work reliably ([Apple Dev Forums #718616](https://developer.apple.com/forums/thread/718616)).

> **`elastos-vz` action (Phase 2 if initrd needed):** if the capsule manifest supplies an initrd, run a Phase-0-style probe before booting to detect compression magic; reject zstd with a clear error pointing the operator at LZ4/gzip. The microVM capsules ElastOS ships today are direct-rootfs and do **not** use initrd, so this is mostly an early-warning rail.

### Pitfall 7 — File-backed kernel-console buffer must be rotated

Lima writes the kernel console to a static file path (`SerialVirtioLog`) using `VZFileSerialPortAttachment(path, false)` ([Lima L297–308](https://github.com/lima-vm/lima/blob/8de0d4a2/pkg/driver/vz/vm_darwin.go#L297)). Without rotation, long-running VMs grow the file unbounded. The crosvm path uses `type=stdout` which the supervisor pipes into a `tracing` target — no file growth issue.

> **`elastos-vz` action (Phase 2):** use `VZFileHandleSerialPortAttachment` over a `socketpair()`/pipe instead of a file, mirror the existing `tracing!` target `vm_console` so the Mac path matches the Linux path's "tracing-only, no on-disk console buffer" behaviour. This matches the current behaviour of [`elastos-crosvm/src/vm.rs:147–168`](../../elastos/crates/elastos-crosvm/src/vm.rs).

### Pitfall 8 — `vmnet` is the underlying NAT engine; do not bind 10.0.0.0/24 host-side

Vz NAT runs on Apple's `vmnet` framework which by default assigns `192.168.64.0/24` or similar; conflicts with any host service that explicitly binds those addresses. Lima documents this implicitly through its `gvisor-tap-vsock` workaround, but ElastOS does not need the gvisor layer because the runtime traffics over vsock + virtio-console, not over the guest's IP stack.

> **`elastos-vz` action (Phase 3 + docs):** document the `vmnet`-assigned subnet in `docs/MAC.md` "Carrier port allocation" section so operators know what host-IP range NOT to use for their carrier listeners.

### Pitfall 9 — `VZVirtualMachine.start` is asynchronous; you need a state-change observer

Calling `.start()` returns immediately. The VM transitions through `starting → running` and you only know the boot succeeded when the delegate's `virtualMachine(_:didChangeState:)` reports `.running`. Lima wires a Go channel from this delegate ([Lima L99–157](https://github.com/lima-vm/lima/blob/8de0d4a2/pkg/driver/vz/vm_darwin.go#L99)).

> **`elastos-vz` action (Phase 2):** wrap the `VZVirtualMachineDelegate` into a `tokio::sync::mpsc` channel; `RunningVm::start` does not return until the first `.running` state notification arrives (or a timeout). Mirrors the crosvm-side "boot complete" signal.

### Pitfall 10 — Vz requires Grand Central Dispatch on the calling thread

Vz APIs assume a `dispatch_queue` for delegate callbacks. The `objc2-virtualization` bindings let you pass a `dispatch2::DispatchQueue`. Without setting a queue, completion handlers race against the runtime's Tokio thread pool and may run on an arbitrary thread.

> **`elastos-vz` action (Phase 1):** create one `dispatch_queue` per `VzProvider` instance, parked on a dedicated Tokio task; route all Vz delegate completions through it.

---

## E. Risk register update

Three new rows and one severity reclassification to apply to [`PLAN.md` Risk register](PLAN.md):

| Risk | Severity | Detected in phase | Fallback |
|---|---|---|---|
| **NEW** — Shipped `vmlinux` on `linux-arm64` uses host-kernel-copy strategy; no content-addressed arm64 artifact exists for Mac Vz to fetch | Medium | Phase 6 | Phase 2 prototypes with Ubuntu cloud arm64 kernel; Phase 6 picks one of {build same 6.1.59 source for arm64, pin Ubuntu LTS arm64 checksum} — both are deliverable in <1 week |
| **NEW** — Initrd compression compatibility (zstd ↔ Vz). Most microVM capsules in the repo today are direct-rootfs without initrd; future capsules may regress here | Low | Phase 2 / future | Detect compression magic; reject zstd with a clear error; document LZ4/gzip recommendation in `docs/MAC.md` |
| **NEW** — `elastos-server/src/vm_provider.rs` uses Linux-only `AF_VSOCK` directly. Adding a Mac arm is allowed under the Linux-untouched gate (file is outside protected crate list) but expands Phase 3 surface area by ~80 LOC | Medium | Phase 3 | Plan accommodates: add a `cfg(target_os = "macos")` arm at L81–85 that opens a Vz `VZVirtioSocketConnection` instead of an `AF_VSOCK` socket. Linux path at L177 stays byte-identical |
| **RECLASSIFY** — *"Vsock semantic deltas vs Linux (port allocation, half-close)"* — was Medium; downgrade to **Low** because we now know exactly what the delta is (no CID API, host-side connection per-VM) and the shim layer is bounded | Low | Phase 3 | (unchanged from existing plan) |

These updates will be applied to [`PLAN.md`](PLAN.md) in the same commit that introduces this file.

---

## F. Honest unknowns

Three items Phase 0 could not resolve from desk research. None blocks proceeding to Phase 1; each is the smallest feasible Phase 1/2 experiment.

1. **Does the current `linux-amd64` ElastOS 6.1.59 vmlinux artifact have `CONFIG_VIRTIO_VSOCKETS=y`?** Without source access to the build pipeline, can only verify by booting it and `lsmod | grep vsock` (or by running `strings vmlinux | grep -i vsock`). The runtime's crosvm path uses `AF_VSOCK` from [`vm_provider.rs:177`](../../elastos/crates/elastos-server/src/vm_provider.rs) so this MUST be enabled or the existing Linux supervisor path would already be broken — so the answer is almost certainly yes. Verify by experiment in Phase 1 / 2 as cheap insurance.
2. **Does the 6.1.59 kernel boot cleanly under `VZGenericPlatformConfiguration` without source patches?** Lima boots arbitrary modern Linux on Vz, so this is *expected* to work, but mainline Linux's Vz support has occasionally required patches (e.g. for clocksource and IRQ controller compatibility on older versions). Verify by Phase 2 first-boot.
3. **Boot latency target.** No public benchmark numbers for `objc2-virtualization`-driven Vz boot vs `crosvm` boot. Lima typically reports 1–3 s to userspace on Apple Silicon; crosvm reports 100–500 ms. ElastOS may take a perf hit on Mac. Document honestly in Phase 5 per [`PRINCIPLES.md` #12](../../PRINCIPLES.md); set a Phase 6 baseline target of "boot ≤ 3× the Linux baseline."

---

## G. Go / no-go recommendation

**Go.** Proceed to Phase 1.

One-sentence reasoning, anchored in [`PRINCIPLES.md`](../../PRINCIPLES.md):

> Phase 0 confirms (a) `objc2-virtualization` exposes every Vz class the runtime needs (no API gap), (b) the existing capsule artifacts and kernel config are compatible with Vz with two cfg-gated boot-arg adjustments and one host-side vsock adapter, and (c) no Linux file in the four protected crates needs to change — which together satisfies [`PRINCIPLES.md` #10 *One Canonical Path*](../../PRINCIPLES.md) (one substrate per platform) and the [Linux-untouched gate](PLAN.md) (Anders' code preserved).

---

## Appendix — Phase 1 prelude TODO: 50-line feature-check binary

Deferred from Phase 0 per the 10/10 prompt because it requires an Apple Silicon Mac with the runtime crates linkable, which is Phase 1's build-environment setup task. Phase 1's first day produces this binary:

```rust
// scripts/dev/mac-vz-feature-check/src/main.rs
// Standalone binary, NOT linked into elastos-vz or any runtime crate.
// Purpose: confirm on the engineer's Apple Silicon Mac that the
// objc2-virtualization API surface we depend on is callable and that
// the host can construct (without starting) a Linux-guest VZ config
// that has every device class §B says we need.
//
// Build: cargo run -p mac-vz-feature-check
// Pass: prints "all devices constructible" + macOS version
// Fail: prints the missing device + the error from Vz

// pseudo-checklist the binary asserts:
//   - VZGenericPlatformConfiguration constructs and accepts a
//     VZGenericMachineIdentifier
//   - VZLinuxBootLoader constructs with a dummy kernel path + commandLine
//   - VZVirtioBlockDeviceConfiguration accepts a
//     VZDiskImageStorageDeviceAttachment with caching=Cached, sync=Fsync
//   - VZVirtioConsoleDeviceSerialPortConfiguration with
//     VZFileHandleSerialPortAttachment
//   - VZVirtioConsoleDeviceConfiguration with a 1-element
//     VZVirtioConsolePortConfigurationArray (multi-port; macOS 12+)
//   - VZVirtioSocketDeviceConfiguration
//   - VZNATNetworkDeviceAttachment + VZVirtioNetworkDeviceConfiguration
//     with a VZMACAddress
//   - VZVirtioEntropyDeviceConfiguration
//   - VZVirtioTraditionalMemoryBalloonDeviceConfiguration
//   - VZVirtualMachineConfiguration.validate() returns Ok with the above
//
// Outcome of running this binary moves Phase 1 status from "scaffolded"
// to "ready for first guest boot in Phase 2".
```

This binary is **not** the `elastos-vz` crate itself; it's a one-shot diagnostic the Phase 1 engineer runs on day 1 to confirm reality matches Phase 0's desk-research claims before scaffolding the real crate.
