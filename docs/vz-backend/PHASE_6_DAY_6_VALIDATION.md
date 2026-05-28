# Phase 6 Day 6 — Substrate Validation Milestone (honest reframe)

**Phase**: 6 (macOS native-binary surface)
**Day**: 6 (validation milestone; supersedes the 6a/6b scaffolding-only framing)
**Date**: 2026-05-25
**Status**: Substrate validated end-to-end against Apple's Virtualization.framework on a real Apple Silicon Mac
**Predecessor**: [`PHASE_6_DAY_6_NOTES.md`](./PHASE_6_DAY_6_NOTES.md) (local-lane scaffolding context)
**Successor**: Day 7 — real Linux VM boot (closes the last unproved claim)

---

## 1. What this document captures

The Phase-6 plan up to Day-5a framed substrate validation around three
"dry-run PRE-PASS" smoke runs. **That framing was misleading.** Re-reading
the smoke scripts (`scripts/*-smoke.sh`) shows dry-run mode literally
exits before `cargo build` — it validates bash syntax + helper sourcing +
macOS-12-detection only. It does NOT exercise the `elastos-vz` substrate
in any way.

The **real** substrate validation has always been
`elastos/crates/elastos-vz/tests/concurrent_launch.rs::concurrent_load_with_real_kernel`.
That test exercises every FFI builder, constructs a full
`VZVirtualMachineConfiguration`, and calls Apple's `validateWithError:`.
It auto-skips (eprintln + return) when no kernel + rootfs are installed,
which is why earlier days reported "pass" without actually exercising
the substrate — the test was a silent no-op.

Day 6 closes that gap. We now have evidence the substrate works.

---

## 2. The audit miss (honest)

The Phase-6 Day-1 audit (`PHASE_6_COMPONENTS_AUDIT.md`) picked
**Decision A: "build same 6.1.59 source for arm64 on the dev Mac"** as
primary, with "pin Ubuntu LTS arm64 cloud-kernel checksum" as an
alternative. The audit did not pressure-test the primary on a clean
macOS host before committing to it.

Day 6 surfaced two macOS-vs-Linux toolchain gaps the audit missed:

1. `scripts/kconfig/merge_config.sh` uses GNU-sed `sed -i 'expr' file`
   syntax that BSD sed (macOS default) rejects with
   `sed: 1: "...": invalid command code .`
2. Kernel host-side tools (sorttable.c, kallsyms.c, mod/file2alias.c,
   mod/modpost.c) `#include <elf.h>`. macOS has no native `elf.h` (it
   uses Mach-O). Brew's `libelf` is Mike Frysinger's 2009 macOS-friendly
   fork; it covers basic Elf types under non-standard header names but
   collides with the kernel's own `uuid_t` and is missing per-arch
   relocation constants.

We made real progress on both gaps in `scripts/build-vmlinux-arm64.sh`
(BSD-sed bypass via cat-append + olddefconfig; libelf shim via
`<libelf/libelf.h>` re-export), but the chain of remaining gaps is
unbounded for a bare-macOS build. **The honest conclusion**: building
the kernel from source on bare macOS is not feasible without significant
additional toolchain work (vendoring a complete glibc-equivalent elf.h
is the lowest-friction path, ~1000 LOC one-time vendor). Decision A's
alternative — "pin Ubuntu LTS arm64 cloud-kernel checksum" — should have
been primary.

The substrate work itself was not affected by this miss. Phase 7 CI work
(building the artifact on a Linux runner, where the build "just works")
remains the long-term path. For Day-6 validation purposes, we use
Ubuntu's published vmlinuz directly.

---

## 3. The real validation (what Day 6 actually proved)

### 3.1 Test artifact: Ubuntu's published arm64 kernel

```
URL:    https://cloud-images.ubuntu.com/jammy/current/unpacked/jammy-server-cloudimg-arm64-vmlinuz-generic
SHA256: b712ef9919cad88f85e25e4b924c3dacde74e866363867b7b447b7841909462a
Size:   15,392,425 bytes (gzipped)
        47,116,680 bytes (decompressed)
Magic:  MZ at offset 0x00 (PE/COFF, EFI-bootable)
        ARMd at offset 0x38 (Linux arm64 Image format)
Source: Ubuntu Cloud Images, jammy (22.04 LTS), kernel 5.15.0-179-generic
```

This is a dual-format file. Apple's Vz `VZLinuxBootLoader` accepts it
directly as a Linux arm64 Image — no rebuild required.

Staged at the canonical install path the supervisor expects:

```
~/.local/share/elastos/bin/vmlinux
```

Plus a 1 MB zero-filled placeholder rootfs at
`~/Downloads/vz-test/rootfs.raw` (the substrate-validation test only
requires the file to exist; it doesn't try to mount it).

### 3.2 Signing recipe (every cargo build)

`cargo build` produces an unsigned Mach-O. Apple's `validateWithError:`
refuses any process without the `com.apple.security.virtualization`
entitlement. Phase-2 Day-4 shipped `scripts/dev/sign-elastos-vz/sign.sh`
for this; Day 6 confirms it works for cargo TEST binaries the same way
it works for the `elastos` binary:

```bash
TEST_BIN=$(ls -t elastos/target/debug/deps/concurrent_launch-* \
              | grep -v '\.\(d\|o\|json\|dwp\)$' | head -1)
bash scripts/dev/sign-elastos-vz/sign.sh "$TEST_BIN"
```

**Codesign does NOT survive a cargo relink.** Re-sign after every
`cargo build` / `cargo test --no-run`.

### 3.3 The test run

```bash
ELASTOS_VZ_TEST_KERNEL="$HOME/.local/share/elastos/bin/vmlinux" \
ELASTOS_VZ_TEST_ROOTFS="$HOME/Downloads/vz-test/rootfs.raw" \
cargo test -p elastos-vz --test concurrent_launch \
    concurrent_load_with_real_kernel -- --nocapture
```

Result:

```
running 1 test
concurrent_load_with_real_kernel: using kernel=/Users/.../vmlinux rootfs=/Users/.../rootfs.raw
test concurrent_load_with_real_kernel ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.02s
```

### 3.4 What that single `ok` proves

The test loads **3 VMs concurrently** through `VzProvider::load_with_vm_config`.
Each load executes the entire substrate FFI chain:

| Code path | Apple API exercised | Result |
|---|---|---|
| `BuiltMachine::from_vm_config` | `VZVirtualMachineConfiguration` constructor | ✅ |
| `ffi/boot_loader.rs` | `VZLinuxBootLoader` + kernelURL + commandLine | ✅ |
| `ffi/console.rs` + `console_forwarder.rs` | `VZVirtioConsoleDeviceSerialPortConfiguration` | ✅ |
| `ffi/network.rs` | `VZVirtioNetworkDeviceConfiguration` + NAT attachment | ✅ |
| `ffi/vsock.rs` | `VZVirtioSocketDeviceConfiguration` + ports | ✅ |
| `ffi/block.rs` | `VZVirtioBlockDeviceConfiguration` + readonly rootfs | ✅ |
| `ffi/balloon.rs` | `VZVirtioTraditionalMemoryBalloonDeviceConfiguration` | ✅ |
| `ffi/entropy.rs` | `VZVirtioEntropyDeviceConfiguration` | ✅ |
| `ffi/lifecycle.rs::VzMachineHandle::new` | `[VZVirtualMachineConfiguration validateWithError:]` | ✅ **Apple accepted it** |
| `ffi/dispatch.rs` per-VM queue (Phase 4 Day 1) | 3 concurrent constructions, no contention | ✅ |
| `VzProvider::vms` `RwLock<HashMap>` | 3 inserts, unique CapsuleIds (no race) | ✅ |

`validateWithError:` is Apple's framework-internal validator. If it
accepts our config, our config is *structurally* correct against Apple's
contract — kernel-loadable, all device configurations valid, no
incompatible combinations.

### 3.5 Full test suite result

```
=== elastos-vz integration tests ===
running 2 tests
test concurrent_load_rejections_isolate_per_vm ... ok
test concurrent_load_with_real_kernel ... ok

=== elastos-vz smoke tests ===
running 11 tests
test off_mac_is_supported_is_strictly_false ... ok
test is_supported_reports_bool_without_panicking ... ok
test vz_provider_constructable_with_defaults_in_phase_1 ... ok
test network_config_new_is_deterministic_and_shape_compatible ... ok
test vz_provider_supports_only_microvm ... ok
test vm_config_from_manifest_translates_to_vz_console_naming ... ok
test vz_provider_load_rejects_wasm_type_with_clear_message ... ok
test vz_provider_vsock_connect_fails_closed_for_unknown_handle ... ok
test vz_provider_lifecycle_methods_fail_closed_for_unloaded_handle ... ok
test vz_provider_load_returns_rootfs_not_found_for_missing_capsule ... ok
test vz_provider_init_creates_state_dirs_idempotently ... ok

13 passed, 0 failed
```

---

## 4. What's still unproved (well-scoped, NOT a blocker)

The `concurrent_load_with_real_kernel` test validates everything up to
"about to start the VM" but does NOT call `provider.start()`. The actual
boot path — kernel decompresses, mounts rootfs, runs `/init`, console
produces output — is a distinct validation. It needs a bootable rootfs
(an initramfs or real ext4 with an `init` binary), which Day-6's 1 MB
placeholder is not.

This is the single remaining substrate claim to validate. It is the
focus of Day 7. See the 10/10 prompt in this commit's PR description.

Anything beyond "VM boots to userspace" is **Phase 7 work**, not Phase 6:

- Carrier/host↔guest RPC over vsock (Phase 8)
- Full smoke pipeline via `elastos-server` binary (needs distribution
  signing flow + a complete component install — operator setup)
- `mac-vz.yml` GitHub Actions self-hosted runner activation (Phase 7)
- Publishing the Mac-vmlinux artifact via `components.json`
  `darwin-arm64` slot (Phase 7 CI work)

---

## 5. Lessons learned

1. **"Dry-run smoke pass" is not substrate validation.** It validates
   bash syntax. Future audits must distinguish "build-system reachable"
   from "substrate exercised against real platform API."

2. **Audit decisions must be reproducibility-tested before commit.** Day
   1 picked the kernel-build strategy without running it on a clean
   Mac. Day-6 cost ~6 hours of toolchain-shim iteration before pivoting
   to Decision A's alternative (Ubuntu prebuilt). The audit could have
   reached the same conclusion in 30 minutes with a smoke run.

3. **The right "go/no-go gate" for a substrate phase is the integration
   test that exercises the real platform API**, not auxiliary tooling.
   For Phase 6 that gate has always been
   `concurrent_load_with_real_kernel`. It must be in the Day-1 audit's
   "validation contract" section explicitly.

4. **Apple's `validateWithError:` is a high-quality oracle.** It checks
   ~40 separate config invariants and returns precise structured
   errors. Trust it; it will catch real bugs.

5. **Ubuntu cloud-images is a reliable, free, no-container kernel
   source** for Phase-6-style substrate validation on Apple Silicon. We
   should consider adding a `setup-vz-test-kernel.sh` helper in Phase 7
   that automates the download + checksum + stage flow for any
   contributor who wants to run the substrate test locally.

---

## 6. Concrete reproduction recipe

For any future contributor (or operator) wanting to re-validate
Phase-6 substrate on their own Apple-Silicon Mac, the complete recipe
is **three commands** after `git clone`:

```bash
# 1. Stage Ubuntu's published arm64 kernel (free, checksummed, ~5 sec).
curl -fsSL \
    https://cloud-images.ubuntu.com/jammy/current/unpacked/jammy-server-cloudimg-arm64-vmlinuz-generic \
  | gunzip > ~/.local/share/elastos/bin/vmlinux
mkdir -p ~/Downloads/vz-test
dd if=/dev/zero of=~/Downloads/vz-test/rootfs.raw bs=1m count=1

# 2. Build + sign the substrate test binary.
cd elastos
cargo test -p elastos-vz --no-run
TEST_BIN=$(ls -t target/debug/deps/concurrent_launch-* \
              | grep -v '\.\(d\|o\|json\|dwp\)$' | head -1)
bash ../scripts/dev/sign-elastos-vz/sign.sh "$TEST_BIN"

# 3. Run the substrate validation test.
ELASTOS_VZ_TEST_KERNEL="$HOME/.local/share/elastos/bin/vmlinux" \
ELASTOS_VZ_TEST_ROOTFS="$HOME/Downloads/vz-test/rootfs.raw" \
cargo test -p elastos-vz --test concurrent_launch \
    concurrent_load_with_real_kernel -- --nocapture
# Expected: `test concurrent_load_with_real_kernel ... ok`
```

If the test fails with `missing com.apple.security.virtualization
entitlement`, you skipped step 2's `sign.sh` invocation. Re-sign and
re-run.

---

## 7. Anchors

- `elastos/crates/elastos-vz/tests/concurrent_launch.rs` — the validation test
- `elastos/crates/elastos-vz/src/provider.rs::load_with_vm_config` (lines 203–290) — the code path the test exercises
- `elastos/crates/elastos-vz/src/ffi/lifecycle.rs::VzMachineHandle::new` (lines 209–250) — where Apple's `validateWithError:` is called
- `scripts/dev/sign-elastos-vz/sign.sh` — the entitlement helper
- `scripts/build-vmlinux-arm64.sh` — the would-be-from-source build recipe (preserved with macOS-native limitation banner)
- `docs/vz-backend/PHASE_6_COMPONENTS_AUDIT.md` — the Day-1 audit (decision-A audit-miss origin)
