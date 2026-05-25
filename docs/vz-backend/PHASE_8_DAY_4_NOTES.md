# Phase 8 Day 4 — First real-substrate boot on Mac end-to-end

**Date**: 2026-05-26
**Branch**: `sash/local-test`
**Anchor**: [`PHASE_8_DAY_3_NOTES.md`](./PHASE_8_DAY_3_NOTES.md) (doctor row) / [`PHASE_6_DAY_7_NOTES.md`](./PHASE_6_DAY_7_NOTES.md) (Day-7 boot-to-userspace test)
**Forward link**: Day 5 will wire `elastos run ubuntu-base` end to end.

## TL;DR

The integration test that's been `#[ignore]`'d on Mac and waiting
for a rootfs since Phase 6 — `concurrent_load_with_real_kernel` —
**passes on this Mac**. Three VMs concurrently constructed
`VzConfig` + `VmConfig` against the real Day-2 kernel + rootfs and
all three cleared `validateWithError:`, with three distinct
`CapsuleId`s under contention.

Bonus: `single_vm_boots_to_userspace` (Phase 6 Day 7's boot test)
also passes on this Mac, with the kernel-console tracing
forwarder capturing arm64 Linux printk reaching `Run /init` in
~131 ms of kernel wall-clock — visible boot evidence that the
substrate boots a real Linux kernel inside Apple Vz.

**3 / 3 elastos-vz integration tests pass on Mac.** Zero
regressions in the 388 elastos-server lib + 95 elastos-vz lib
tests.

## What shipped (one commit)

### `tests/concurrent_launch.rs` — platform-aware data-dir resolution

Three discover helpers (`discover_kernel`, `discover_initrd`,
`discover_rootfs`) were hard-coding `~/.local/share/elastos/`. On
Mac this directory doesn't exist; `elastos setup` installs to
`~/Library/Application Support/elastos/` (the `dirs::data_dir()`
return on macOS). Result: every Mac run of the test silently
skipped via `eprintln` + `return`, so the "is the substrate ready"
acceptance bar has never actually been exercised on Mac.

Fix in three parts:

1. **New `test_data_dir()` helper** (3 lines) returning
   `dirs::data_dir().map(|d| d.join("elastos"))`. Linux: same
   `~/.local/share/elastos` as before (byte-identical Linux
   behaviour). macOS: `~/Library/Application Support/elastos`.
2. **`discover_kernel` / `discover_rootfs`**: read base from
   `test_data_dir()` instead of `HOME + .local/share`. Env-var
   overrides (`ELASTOS_VZ_TEST_KERNEL`, `ELASTOS_VZ_TEST_ROOTFS`)
   short-circuit before the platform-default lookup — operators
   pointing at non-default paths keep that escape hatch.
3. **`discover_initrd`**: same data-dir fix PLUS a second latent
   bug fixed. Phase 7 Day 2 standardised the canonical filename to
   `bin/initrd` (no suffix); the discover helper was still looking
   for `bin/initrd-generic`. The new code checks the canonical
   name first, falls back to the legacy name for any operator who
   still has the older artefact on disk.

### `Cargo.toml`

Added `dirs = "5.0"` as a dev-dep. Pinned to match the workspace
norm (`elastos-server`, `elastos-storage`, `elastos-compute` all
on 5.0). `elastos-vz` runtime deps are unchanged — `dirs` is
test-crate-only, so the production substrate stays slim.

### Skip-message updates

The two tests' skip messages still referenced the hard-coded path.
Updated to read `<data_dir>` (and to standardise on `elastos setup
--profile minimal` as the single remediation, matching the
`elastos doctor` Day-3 row).

## Acceptance bar — all met

- [x] `discover_kernel` / `discover_initrd` / `discover_rootfs`
      resolve through `dirs::data_dir()` (Linux byte-identical to
      old behaviour; macOS now finds `elastos setup` installs).
- [x] `ELASTOS_VZ_TEST_KERNEL` / `_INITRD` / `_ROOTFS` env-var
      overrides remain functional.
- [x] `concurrent_load_with_real_kernel` passes against the Day-2
      installed kernel + rootfs on this Mac.
- [x] `single_vm_boots_to_userspace` passes against the Day-2
      installed kernel + Day-2 installed initrd — kernel-console
      tracing captures the printk marker `Run /init`, confirming
      a real Linux userspace handover inside Vz.
- [x] `cargo test -p elastos-server --lib`: **388/388** (no
      regressions; doctor's Day-3 paired tests still green).
- [x] `cargo test -p elastos-vz --lib`: **95/95** (no regressions).
- [x] Manual integration smoke: 3/3 tests in `concurrent_launch`
      binary pass on Mac.
- [x] One commit, one notes file.

## Smoke-test log (verbatim, this Mac)

```text
$ "$TEST_BIN" --nocapture
running 3 tests
single_vm_boots_to_userspace: kernel=/Users/sash/Library/Application Support/elastos/bin/vmlinux initrd=/Users/sash/Library/Application Support/elastos/bin/initrd rootfs=/var/folders/sb/c8qcp67d6nxfpx6ypcdn0lfr0000gn/T/elastos-vz-day7-rootfs.raw
concurrent_load_with_real_kernel: using kernel=/Users/sash/Library/Application Support/elastos/bin/vmlinux rootfs=/Users/sash/Library/Application Support/elastos/capsules/ubuntu-base/rootfs.ext4
test concurrent_load_rejections_isolate_per_vm ... ok
test concurrent_load_with_real_kernel ... ok
=== first ≤30 kernel-console lines ===
    [    0.126089] cacheinfo: Unable to detect cache hierarchy for CPU 0
    [    0.126633] loop: module loaded
    ...
    [    0.128658] Loaded X.509 cert 'Canonical Ltd. Live Patch Signing 2025 Kmod: ...'
=== end console capture ===
single_vm_boots_to_userspace: PASS (marker 'Run /init' observed)
test single_vm_boots_to_userspace ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.29s
```

### What `concurrent_load_with_real_kernel` actually proves

- Three VMs concurrently constructed `VzConfig` + `VmConfig` against
  real artefacts.
- All three cleared Apple's `validateWithError:` — kernel exists +
  loads, rootfs file is acceptable to Vz's
  `VZDiskImageStorageDeviceAttachment` (the squashfs we shipped on
  Day 2 is a sequence of bytes Vz happily attaches as a block
  device, regardless of the guest-side filesystem; squashfs decoding
  happens guest-side once the kernel sees /dev/vda).
- Three distinct `CapsuleId`s minted under contention — the per-VM
  dispatch queue refactor (Phase 4 Day 1) survives N≥3 concurrent
  loads against one provider.
- 0.01s wall-clock for the three concurrent loads on this Mac.

### What `single_vm_boots_to_userspace` actually proves

- The same kernel boots end to end inside Vz (`vm_console` capture
  shows printk reaching `[ 0.128xxx]` past Canonical X.509 cert
  loading).
- The boot marker `Run /init` was observed — i.e. the kernel handed
  over to the initramfs `/init`. (Userspace was reached, not just
  early kernel init.)
- The rootfs the kernel sees in this particular test is a 1 MB
  zero-filled placeholder (`temp_dir()/elastos-vz-day7-rootfs.raw`),
  NOT the Day-2 squashfs. That's by design — when an initramfs is
  attached, the initramfs IS the rootfs and the kernel never needs
  to mount /dev/vda. The Day-2 squashfs is exercised by
  `concurrent_load_with_real_kernel` (as a Vz-accepted disk image)
  and will be exercised as the actual root filesystem in Day-5's
  `elastos run` path with `root=/dev/vda rootfstype=squashfs`.

## Process notes

### Two test binaries in `target/debug/deps/`

Cargo sometimes mints two `concurrent_launch-<hash>` binaries
across invocations from different working directories (one from
`cargo test` run at `/elastos`, one from a `cargo build` run at
the repo root, even with identical sources). Codesign attaches to
the inode, not the source — so signing whichever binary is on disk
first does not guarantee cargo will execute that one on the next
run.

Robust pattern that worked here:

```bash
TEST_BIN=$(cargo test --no-run --message-format=json ... \
  | jq -r 'select(.target?.name=="concurrent_launch") | .executable | strings' \
  | tail -1)
bash scripts/dev/sign-elastos-vz/sign.sh "$TEST_BIN"
"$TEST_BIN" --nocapture
```

Parse the artefact path from cargo's JSON output, sign that exact
path, then **execute the binary directly** instead of round-tripping
through another `cargo test` invocation that might recompile it.

This isn't a new finding — it's been a recurring trap since Phase
2 Day 4. Documented here so the next contributor doesn't lose 30
min to it.

### `concurrent_load_with_real_kernel` is NOT `#[ignore]`'d

Initial prompt and my first attempt at running the test used
`cargo test ... -- --ignored`. That returned `running 0 tests; 3
filtered out`. The test is `#[tokio::test]` with no `#[ignore]` —
it skips via runtime `eprintln + return` when artefacts are
missing. Re-run without `--ignored` and the runner discovered it
correctly. Updated my mental model.

## Deferred items (NOT Day-4 scope)

- **Pre-existing verifier drift** still open (`scripts/lib/components-json-verify.sh`
  Class-C check). Orthogonal cleanup; Day-2/3 already noted.
- **`elastos run ubuntu-base` end-to-end**: Day 5. Now that the
  substrate is proven to boot a real kernel + accept the real
  rootfs, we wire up the high-level CLI that combines them into
  the v0.1-demo "real ElastOS workload on Mac" experience.

## Why this matters

Phase 6 Day 7 proved the substrate could boot Ubuntu's kernel
through an initramfs. Phase 8 Days 1–4 prove the same substrate
can do it from artefacts the operator installed through one
command (`elastos setup --profile minimal`), discovered through
platform-aware resolution, and verified by a tool the operator
can run on their own machine (`elastos doctor`). The substrate is
operator-ready. The remaining gap to "real workload on Mac" is
purely the high-level CLI wiring — that's Day 5.

## Next

Phase 8 Day 5 — wire `elastos run ubuntu-base` end to end. Likely
involves shipping a minimal `capsule.json` stub at
`<data_dir>/capsules/ubuntu-base/` (the supervisor's
`ensure_capsule` gates on `capsule.json.is_file()`) and wiring
`MicroVmConfig.boot_args` to include `root=/dev/vda
rootfstype=squashfs` so the kernel knows to mount the rootfs once
the initramfs hands over.
