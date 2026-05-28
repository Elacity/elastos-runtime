# Phase 2 Day 5 — first real boot

Day 4 closed with every byte of the Vz lifecycle wired and a
`vm-debug boot` that reached Apple's `startWithCompletionHandler:`
before failing with the opaque *"Internal Virtualization error.
The virtual machine failed to start."* The Day 4 outcome log
([`PHASE_2_DAY_4_NOTES.md`](PHASE_2_DAY_4_NOTES.md)) attributed
that failure exclusively to "kernel artifact incompatibility";
Day 5 confirms that diagnosis was correct by replacing the
synthetic kernel + rootfs with real artifacts and observing
a full Ubuntu boot to systemd.

## TL;DR

> **Apple Silicon now boots a real Linux guest under
> `elastos-vz`.** Kernel reaches userspace, EXT4 mounts, systemd
> initialises, Ubuntu 22.04 announces itself, all kernel printk
> output streams through the `vm_console` tracing target. The
> guest pauses where every cloud-init-driven Linux VM pauses
> when nobody hands it metadata (`Wait for cloud-init`), which
> is a userspace contract, not a virtualisation defect.

## Day 5 deliverables

| File | Change |
|---|---|
| [`elastos-vz/src/config.rs`](../../elastos/crates/elastos-vz/src/config.rs) | `VmConfig.initramfs_path: Option<PathBuf>` + `with_initramfs_path` builder. `VzConfig` gains the same field as a provider-wide default. |
| [`elastos-vz/src/ffi/boot_loader.rs`](../../elastos/crates/elastos-vz/src/ffi/boot_loader.rs) | `build_boot_loader` accepts `Option<&Path>` and calls `VZLinuxBootLoader.setInitialRamdiskURL:` when present. Three new tests assert: (a) `None` leaves the ramdisk URL `nil`, (b) `Some(present)` round-trips through Apple's NSURL copy, (c) `Some(missing)` returns a typed not-found error matching the existing kernel-not-found shape. |
| [`elastos-vz/src/ffi/builder.rs`](../../elastos/crates/elastos-vz/src/ffi/builder.rs) | Plumbs `vm.initramfs_path` to `build_boot_loader`. One new threading-correctness test walks `BuiltMachine → bootLoader → downcast<VZLinuxBootLoader> → initialRamdiskURL` to confirm the field reaches Apple, not just the Rust intermediate. |
| [`elastos-vz/src/provider.rs`](../../elastos/crates/elastos-vz/src/provider.rs) | `VzProvider::load` copies the provider-wide initramfs default onto each `VmConfig` when the per-VM field is `None`. The `is_none()` precondition keeps the door open for a future per-VM override. |
| [`elastos-server/src/vm_debug_cmd.rs`](../../elastos/crates/elastos-server/src/vm_debug_cmd.rs) | `--initramfs <path>` clap flag; canonicalised + validated; threaded into `VzConfig::with_initramfs_path`. Two new tests cover validation. |
| [`scripts/dev/fetch-vz-kernel.sh`](../../scripts/dev/fetch-vz-kernel.sh) | Idempotent macOS-only fetcher. Downloads Ubuntu 22.04 arm64 cloud-image artifacts (kernel + initrd + qcow2 disk) from an archived `release-YYYYMMDD/` path, verifies SHA-256 against checksums baked into the script, gunzips the kernel to a raw arm64 Image, and converts the disk from qcow2 to raw via `qemu-img`. `--verify-only` re-checksums without downloading; `--force` clobbers. |
| [`scripts/dev/sign-elastos-vz/sign.sh`](../../scripts/dev/sign-elastos-vz/sign.sh) | Default binary path corrected from `target/debug/elastos` → `elastos/target/debug/elastos` so the documented recipe (`sign.sh` with no args) works. Day 4 introduced this bug; Day 5 fixes it as a one-line polish since the new MAC.md recipe relies on the default. |
| [`docs/MAC.md`](../MAC.md) | "First boot on Apple Silicon" section updated for the new four-step recipe: `cargo build → sign.sh → fetch-vz-kernel.sh → vm-debug boot`. Common error shapes documented. |

## What we did

1. Added `setInitialRamdiskURL:` support to `ffi/boot_loader.rs`,
   threaded it through `ffi/builder.rs`, exposed it on `VmConfig`
   and `VzConfig`. Verified end-to-end with a threading test that
   downcasts the `VZBootLoader` Apple hands back to a
   `VZLinuxBootLoader` and reads the stored URL.
2. Added the `--initramfs <path>` flag to `elastos vm-debug boot`,
   with the same `does-the-file-exist` discipline already applied
   to `--rootfs` and `--kernel`.
3. Wrote `scripts/dev/fetch-vz-kernel.sh` pinned to Ubuntu's
   archived `release-20260515` for jammy 22.04 (immutable release
   path, perfect for baked-in checksums). Verified all three
   SHA-256 hashes against Ubuntu's own `SHA256SUMS` files. The
   script does the kernel decompression and qcow2→raw conversion
   inline so the operator gets one command to run.
4. Built the binary, signed it with the codesign helper, fetched
   the artifacts, ran `elastos vm-debug boot` against them.

## First-boot evidence (verbatim excerpts)

`vm-debug boot --rootfs … --kernel … --initramfs … --memory-mb 1024 --boot-args 'console=hvc0 root=/dev/vda1 rw'`:

```
vm-debug boot: loading capsule
2026-05-22T07:29:38.701071Z  INFO elastos_vz::provider: Loaded MicroVM capsule 'vm-debug-boot' with ID microvm-71355ae7-…
vm-debug boot: capsule loaded (microvm-71355ae7-…); starting…
vm-debug boot: guest started. Press Ctrl-C to stop. Guest kernel console streams via tracing target `vm_console`.
```

Kernel boot (first lines from the guest's `/dev/hvc0`, forwarded
through our `ConsoleForwarder` to the `vm_console` tracing
target):

```
[    0.140416] cacheinfo: Unable to detect cache hierarchy for CPU 0
[    0.140951] loop: module loaded
[    0.141149] tun: Universal TUN/TAP device driver, 1.6
[    0.141288] ehci-pci: EHCI PCI platform driver
[    0.142044] NET: Registered PF_INET6 protocol family
…
[    0.146484] Loaded X.509 cert 'Canonical Ltd. Secure Boot Signing (Ubuntu Core 2019): c1d57b8f…'
```

Rootfs mounted from the disk image:

```
[    2.513350] EXT4-fs (vda1): mounted filesystem with ordered data mode. Opts: (null). Quota mode: none.
```

Userspace alive, Ubuntu detects the host correctly:

```
[    2.597310] systemd[1]: systemd 249.11-0ubuntu3.20 running in system mode …
[    2.597422] systemd[1]: Detected virtualization vm-other.
[    2.597453] systemd[1]: Detected architecture arm64.
                Welcome to Ubuntu 22.04.5 LTS!
[    2.597893] systemd[1]: Hostname set to <ubuntu>.
[    2.672244] systemd[1]: Queued start job for default target Graphical Interface.
```

The guest then enters cloud-init's `Wait for cloud-init Network
to be Configured` job, which would normally hang for the full
default timeout (~5 min) and then proceed to a login prompt
that no one is reading. We sent SIGINT after ~2 min of confirmed
userspace activity; `provider.stop` cleanly transitioned the VM
through Apple's Stopping → Stopped sequence.

## What this proves

Every Day 1–5 piece of work is operational end-to-end:

| Component | Status |
|---|---|
| `is_supported()` returning `true` on Apple Silicon | ✅ Day 1 |
| `VzProvider::new` and `init` | ✅ Day 1 |
| FFI builder assembling a complete `VZVirtualMachineConfiguration` | ✅ Day 2 |
| Per-provider serial GCD queue | ✅ Day 2 |
| Persistent `VZGenericMachineIdentifier` under the state dir | ✅ Day 2 |
| `VZVirtualMachineConfiguration.validateWithError:` passing | ✅ Day 3 |
| GCD-bridged async lifecycle (`start` / `stop`) | ✅ Day 3 |
| Console pipe + `vm_console` tracing forwarder | ✅ Day 3 |
| `com.apple.security.virtualization` entitlement | ✅ Day 4 |
| `elastos vm-debug boot` CLI + capsule staging | ✅ Day 4 |
| `setInitialRamdiskURL:` for distro kernels | ✅ Day 5 |
| Real arm64 Linux kernel + initramfs + rootfs | ✅ Day 5 |
| Guest reaches userspace (EXT4 mount + systemd up) | ✅ Day 5 |

The Linux kernel's identification of the hypervisor as
`vm-other` confirms it sees Apple's hypercall surface but
not the QEMU / KVM markers it would otherwise advertise. That
is the correct Vz answer.

## What Day 5 deliberately did NOT do

These were called out as out-of-scope in the Day 5 prompt and
remain that way:

1. **`VZVirtualMachineDelegate`** for richer guest-stop
   diagnostics. The polling-on-`current_state` + console
   forwarder + Apple's completion-handler-on-start trio is
   sufficient for this milestone. The delegate will land in
   Day 6 or Phase 3 when we need it (e.g. surfacing
   guest-side panic strings without parsing the console
   stream).
2. **More than one kernel source variant.** The fetch script
   pins Ubuntu jammy `release-20260515`. If Ubuntu ever
   rotates the release path or stops the archive, Day 6 swaps
   in a different known-good source (Debian, Alpine, NixOS,
   or a hand-built Yocto image). The wiring (`--initramfs`,
   the SHA-verify loop) is artifact-agnostic.
3. **Cloud-init bypass.** The guest's pause on
   `Wait for cloud-init Network to be Configured` is the
   expected behaviour for a cloud image booted without
   metadata. Phase 3 of the plan (capsule integration)
   produces capsule rootfs images that don't ship cloud-init
   at all, so this stops being relevant. The Day 5 attempt
   was about proving Vz boots Linux, not about productionising
   an Ubuntu image.
4. **Pre-existing `elastos-server::setup::tests::*` failures.**
   Three tests in `setup.rs` continue to fail with the
   "no platform entry for darwin-arm64" message inherited
   from Pre-Work. They are unrelated to the Vz backend
   (their fix lives inside the `setup` module, which manages
   `components.json`) and were explicitly excluded from
   Phase 2's scope. Day 5 commits introduce zero new test
   failures.

## Reproducible local recipe

```bash
# From the repo root.

# 1. Build.
( cd elastos && cargo build -p elastos-server --bin elastos )

# 2. Sign.
scripts/dev/sign-elastos-vz/sign.sh

# 3. Fetch artifacts (~700 MB download, ~2.2 GB after qcow2→raw).
#    Requires `qemu-img` (brew install qemu).
scripts/dev/fetch-vz-kernel.sh

# 4. Boot.
elastos/target/debug/elastos vm-debug boot \
  --rootfs    ~/.local/share/elastos/vz-bin/rootfs.img \
  --kernel    ~/.local/share/elastos/vz-bin/Image \
  --initramfs ~/.local/share/elastos/vz-bin/initramfs.img \
  --memory-mb 1024 \
  --boot-args 'console=hvc0 root=/dev/vda1 rw'

# Wait for `EXT4-fs (vda1): mounted filesystem` + the systemd
# banner. Press Ctrl-C once the cloud-init wait kicks in.
```

## Day 6 handoff

With first boot in hand, the next milestones — in the order
the plan calls them out — are:

1. **Per-VM initramfs override path** so the supervisor can
   plumb per-capsule artifacts later in Phase 3 without
   touching the provider-wide default.
2. **`VZVirtualMachineDelegate`** for guest-side fault
   surfacing (panic strings, vCPU traps, balloon events).
   The delegate will give us a structured analogue to Apple's
   opaque "Internal Virtualization error" — useful for the
   hardening phase.
3. **Phase 3 (virtio plumbing) entrypoint:** wire the Carrier
   bridge through the multi-port `VZVirtioConsoleDeviceConfiguration`
   slot the builder already reserves at `/dev/hvc1`, replace
   the placeholder NSPipe attachment with a real socketpair,
   and connect to the supervisor's Carrier endpoint.

## Cross-references

- The plan: [`PLAN.md`](PLAN.md)
- Day 0 / context: [`PHASE_0_SCOPE.md`](PHASE_0_SCOPE.md)
- Day 4 outcome log: [`PHASE_2_DAY_4_NOTES.md`](PHASE_2_DAY_4_NOTES.md)
- Operator recipe: [`../MAC.md`](../MAC.md) ("First boot on Apple Silicon")
- Codesign helper: [`../../scripts/dev/sign-elastos-vz/README.md`](../../scripts/dev/sign-elastos-vz/README.md)
- Kernel fetcher: [`../../scripts/dev/fetch-vz-kernel.sh`](../../scripts/dev/fetch-vz-kernel.sh)
