# Phase 2 Day 4 — outcome log

Operator-facing summary of what Day 4 actually achieved on real
hardware. Single source of truth for "what works, what doesn't,
what's next" so Day 5 can pick up without ambiguity. Companion
to [`PLAN.md`](PLAN.md) (high-level phasing) and
[`PHASE_0_SCOPE.md`](PHASE_0_SCOPE.md) (audit / risk register).

## What this commit ships

| Deliverable | Status |
|---|---|
| `scripts/dev/sign-elastos-vz/sign.sh` + `vz.entitlements.plist` + `README.md` | Done — ad-hoc signs the local `elastos` binary with `com.apple.security.virtualization`, idempotent, refuses to run off macOS |
| `elastos vm-debug boot --rootfs … --kernel …` CLI subcommand | Done — `Commands::VmDebug` registered everywhere, real implementation gated to macOS, non-macOS returns typed "see docs/MAC.md" error |
| `vm_console` tracing target opted into the default env filter | Done — single additive `add_directive("vm_console=info"…)` in `elastos-server/src/main.rs`; `RUST_LOG` still overrides |
| End-to-end first boot attempt on this dev machine | **Attempted; outcome documented below** |

## End-to-end attempt result

**Host:** macOS 26.4.1 / aarch64 (Apple Silicon).

**Procedure executed:**
1. `cargo build -p elastos-server`
2. `scripts/dev/sign-elastos-vz/sign.sh elastos/target/debug/elastos`
3. `scripts/dev/sign-elastos-vz/sign.sh --verify-only elastos/target/debug/elastos` —
   confirmed `com.apple.security.virtualization = true`.
4. Created a stand-in 1 MiB zero-filled "rootfs" + an 18-byte
   "kernel" stand-in (`/tmp/fake-rootfs.img`, `/tmp/fake-kernel.bin`).
5. `elastos/target/debug/elastos vm-debug boot --rootfs /tmp/fake-rootfs.img --kernel /tmp/fake-kernel.bin --memory-mb 128`

**Verbatim output:**

```text
vm-debug boot: loading capsule
2026-05-22T06:42:40.529620Z  INFO elastos_vz::provider: Loaded MicroVM
  capsule 'vm-debug-boot' with ID microvm-9c5055ad-cebe-4cbf-a49c-ad69945b0e63
vm-debug boot: capsule loaded (microvm-9c5055ad-cebe-4cbf-a49c-ad69945b0e63); starting…
Error: vm-debug boot: provider.start: Compute error:
  vz start (vm_id='0d7f43ac-a550-454d-ade0-b58a73f7bb74'):
  Internal Virtualization error. The virtual machine failed to start.
```

**Interpretation (line by line):**

1. `vm-debug boot: loading capsule` — the CLI dispatch reached
   `provider.load`. The staging-dir layout + manifest synth
   worked end-to-end.
2. `Loaded MicroVM capsule … ID microvm-…` — `VzProvider::load`
   returned `Ok(CapsuleHandle)`. This implies all of:
   - `ffi::builder::BuiltMachine::from_vm_config` accepted the
     inputs (rootfs + kernel paths exist, sizing valid).
   - `VZVirtualMachineConfiguration.validateWithError`
     **succeeded** — the entitlement is real, the per-device
     config the Day 2 builder produced is acceptable to Apple
     for the host's macOS version, and the Day 1 probe's
     extrapolation held.
   - `VZVirtualMachine::initWithConfiguration:queue:` succeeded
     — the per-provider GCD queue and the SendableVm wrapper
     are correctly wired.
3. `vz start (vm_id='…'): Internal Virtualization error. The
   virtual machine failed to start.` — this is Apple's response
   when it tries to actually boot the kernel image and finds
   it isn't a valid Linux Image. **Expected** for an 18-byte
   stand-in.

**Conclusion.** The Phase 1–3 lifecycle wiring (validate →
init → start → completion-handler block → Tokio oneshot →
typed Rust error) is end-to-end functional on a signed Apple
Silicon binary. The only thing Day 4 cannot verify is the
actual guest boot, because we did not procure a real arm64
Linux Image + matching rootfs.

## What this rules out

- ❌ Missing entitlement (would surface as the Day 3
  `ENTITLEMENT_HINT` message; we don't see it).
- ❌ Config-validation rejection (would surface from
  `validateWithError` with a specific configuration error;
  we don't see it).
- ❌ GCD queue / dispatch wiring (the start call reached Apple
  and Apple's framework reported back through our completion
  handler).
- ❌ FFI builder problem (the BuiltMachine assembled cleanly
  for a config with both a kernel file and a rootfs file).

## What's left for Day 5

1. **Procure a Vz-compatible arm64 Linux kernel + minimal
   rootfs.** Candidates ranked by effort:
   - Apple's developer sample code under
     `https://developer.apple.com/documentation/virtualization/running_linux_in_a_virtual_machine`
     ships a known-working pair.
   - Lima's published kernel + initramfs releases at
     `https://github.com/lima-vm/alpine-lima/releases` are
     reliable for arm64.
   - Build from upstream: `arch/arm64/configs/defconfig` plus
     `CONFIG_VIRTIO_BLK`, `CONFIG_VIRTIO_NET`, `CONFIG_HVC_DRIVER`,
     `CONFIG_VIRTIO_CONSOLE` — see Phase 0 audit §C "kernel
     config" for the full list.
2. **First-boot acceptance test.** Run `vm-debug boot` against
   the real artifacts; observe `vm_console` lines like
   `[ 0.000000] Linux version …` reach stdout via the tracing
   forwarder; clean Ctrl-C stop; verify no leftover state in
   the tempdir.
3. **VZVirtualMachineDelegate.** Once boot works, surface
   guest-stop reasons and crash details via the delegate
   protocol (Phase 0 §D pitfall #9; Day 3 deferred this).

## Reproducing this attempt locally

```bash
# Sign the binary.
cargo build -p elastos-server
scripts/dev/sign-elastos-vz/sign.sh

# Build stand-in artifacts so the "infrastructure works"
# assertion is reproducible without a real kernel.
dd if=/dev/zero of=/tmp/fake-rootfs.img bs=1M count=1
echo "fake-kernel-bytes" > /tmp/fake-kernel.bin

# Drive the lifecycle. Expected exit 1 with Apple's "Internal
# Virtualization error" once we reach `provider.start`.
target/debug/elastos vm-debug boot \
  --rootfs /tmp/fake-rootfs.img \
  --kernel /tmp/fake-kernel.bin \
  --memory-mb 128
```

## Anchors

- [`PLAN.md`](PLAN.md) — Phase 2 Day 4 in the phasing table
- [`PHASE_0_SCOPE.md`](PHASE_0_SCOPE.md) §B, §D pitfalls #9, #10
- [`scripts/dev/sign-elastos-vz/README.md`](../../scripts/dev/sign-elastos-vz/README.md)
- [`docs/MAC.md`](../MAC.md) — operator recipe
- `elastos-vz/src/ffi/lifecycle.rs::ENTITLEMENT_HINT` — the
  string that does **not** appear in the Day 4 attempt above,
  confirming the codesign path works
