# mac-vz-feature-check

Standalone Apple Virtualization.framework reality probe — Phase 2 Day 1
deliverable of [`docs/vz-backend/PLAN.md`](../../../docs/vz-backend/PLAN.md).

This binary is **not** part of the runtime build and **not** linked into
`elastos-vz`. Its only job is to convert every desk-research claim in
[`docs/vz-backend/PHASE_0_SCOPE.md`](../../../docs/vz-backend/PHASE_0_SCOPE.md)
§B (feature-coverage table) and §D (pitfalls) into runtime-verified fact
on an actual Mac, **before** Phase 2 main writes a single line of
production Vz code in `elastos-vz/src/ffi/`.

## Run

```bash
cargo run --manifest-path scripts/dev/mac-vz-feature-check/Cargo.toml
```

Build cost: ~6 seconds cold, ~0.2 seconds warm. Runtime: <1 second.

Requires:

- Apple Silicon Mac (aarch64) — Phase 6 ships `darwin-arm64` only;
  Intel Mac is documented as a future deliverable.
- macOS 12.0 or newer — Vz Linux-guest support matured here, and the
  multi-port virtio-console probe (`console-mp`) needs macOS 12+.
- Xcode command-line tools (any Rust dev environment on Mac has these).

No code signing or entitlements are needed to run the probe; see the
**validate** line below for why that matters.

## What it probes

Each probe maps 1:1 to a row in [`PHASE_0_SCOPE.md`](../../../docs/vz-backend/PHASE_0_SCOPE.md)
§B or pitfall in §D. Order matches the
[`PLAN.md` Phase 2 main work](../../../docs/vz-backend/PLAN.md).

| Probe | Phase 0 anchor | Asserts |
|---|---|---|
| `host` | §C | macOS version, Apple Silicon detection |
| `dispatch` | §D #10 | `dispatch_queue` constructible (serial) |
| `identifier` | §D #2 | `VZGenericMachineIdentifier` + `dataRepresentation` round-trip |
| `platform` | §B | `VZGenericPlatformConfiguration` + identifier attached |
| `boot-loader` | §D #3 | `VZLinuxBootLoader` with `console=hvc0` cmdline |
| `storage` | §D #1 | `VZDiskImageStorageDeviceAttachment` with `cachingMode=Cached`, `synchronizationMode=Fsync` (UTM #4840) |
| `console` | §D #7 | Pipe-backed `VZFileHandleSerialPortAttachment` for kernel console |
| `console-mp` | §D #4 | Multi-port `VZVirtioConsoleDeviceConfiguration` (Carrier-bridge slot for Phase 3) |
| `vsock` | §D #5 | `VZVirtioSocketDeviceConfiguration` (no CID API) |
| `network` | §B | `VZNATNetworkDeviceAttachment` + random locally-administered `VZMACAddress` (no entitlement) |
| `entropy` | §B | `VZVirtioEntropyDeviceConfiguration` |
| `balloon` | §B | `VZVirtioTraditionalMemoryBalloonDeviceConfiguration` |
| `validate` | §D #0 / Phase 6 | Full `VZVirtualMachineConfiguration.validateWithError` on the assembled config |

## How to read the output

Each line is `name: STATUS — detail`:

- `OK` — the probe constructed the device class cleanly. This is what
  we care about; it means `elastos-vz` Phase 2 main can lift the same
  pattern into `elastos-vz/src/ffi/`.
- `SKIP` — the probe is informational only and does not affect exit
  code. The most common SKIP is the `validate` line on unsigned dev
  builds (see below).
- `FAIL` — actual API surface mismatch or unexpected error. Phase 2
  main must not start until the failure is understood and either fixed
  or documented in `PHASE_0_SCOPE.md`.

Exit code:

- `0` — no `FAIL` observed; every device class is constructible.
- `1` — at least one `FAIL`. Stop and reconcile with `PHASE_0_SCOPE.md`.
- `2` — binary executed on a non-macOS host (not applicable).

## The `validate` line is expected to SKIP on dev builds

Apple requires the `com.apple.security.virtualization` entitlement for
`VZVirtualMachineConfiguration.validateWithError` to succeed.
[Apple's docs](https://developer.apple.com/documentation/virtualization/vzvirtualmachineconfiguration)
state plainly:

> Creating a virtual machine using the Virtualization framework requires
> the app to have the "com.apple.security.virtualization" entitlement.
> A `VZVirtualMachineConfiguration` is considered invalid if the
> application does not have the entitlement.

A `cargo run`'d binary on a dev Mac without code signing **will not**
have that entitlement. The probe reports this case as `SKIP` with the
actual Apple `NSError.localizedDescription` captured verbatim — that
diagnostic is the signal driving the
[Phase 6 release-pipeline work](../../../docs/vz-backend/PLAN.md)
(code-sign + notarize + entitlement plist). Treat a `SKIP` on the
`validate` line as **expected and informational**; treat any other
status as a real signal.

If you want to see `validate: OK`, sign the binary locally:

```bash
codesign --entitlements scripts/dev/mac-vz-feature-check/entitlements.plist \
         --sign - \
         scripts/dev/mac-vz-feature-check/target/debug/mac-vz-feature-check
```

(`entitlements.plist` is not in the repo today — that's Phase 6 work.)

## What this binary does NOT do

- It does **not** start a VM. Phase 2 main does that, anchored to the
  patterns this binary verifies.
- It does **not** boot a kernel. The kernel/rootfs paths are temp
  files; they are passed to constructors but never read because the
  binary never calls `VZVirtualMachine.start`.
- It does **not** touch `elastos-vz/` or any of the Linux-untouched
  protected crates ([`elastos-crosvm/`](../../../elastos/crates/elastos-crosvm/),
  [`elastos-runtime/`](../../../elastos/crates/elastos-runtime/),
  [`elastos-common/`](../../../elastos/crates/elastos-common/),
  [`elastos-compute/`](../../../elastos/crates/elastos-compute/)).
  Linux-untouched gate passes trivially.

## Why we wrote this before any FFI code in elastos-vz

Per [`PRINCIPLES.md` #11 *Fail Closed, Then Explain*](../../../PRINCIPLES.md)
and #12 *Docs, Code, Tests, and Ops Must Agree*: every line of Vz code
in `elastos-vz/src/ffi/` written before this binary passes is
speculative. If the `objc2-virtualization` API surface differs from the
desk research in [`PHASE_0_SCOPE.md`](../../../docs/vz-backend/PHASE_0_SCOPE.md),
the cheapest place to discover that is here — in a single throwaway
binary — not in committed code that has to be reworked.

When this binary reports `PHASE_2_DAY_1: ALL DEVICE CLASSES
CONSTRUCTIBLE`, Phase 2 main is unblocked: lift the patterns into
`elastos-vz/src/ffi/{boot_loader, platform, block, console, vsock,
network, entropy, balloon, lifecycle, dispatch}.rs`, anchored row-for-row
in [`PHASE_0_SCOPE.md`](../../../docs/vz-backend/PHASE_0_SCOPE.md) §B.

## Dependencies (pinned to elastos-vz)

| Crate | Version | Source |
|---|---|---|
| `objc2` | `0.6` | matches `elastos-vz/Cargo.toml` |
| `objc2-virtualization` | `0.3` | matches `elastos-vz/Cargo.toml` |
| `objc2-foundation` | `0.3` | matches `elastos-vz/Cargo.toml` |
| `dispatch2` | `0.3` | matches `elastos-vz/Cargo.toml` |
| `tempfile` | `3.10` | probe-only (creates fixture kernel + rootfs files) |
| `libc` | `0.2` | probe-only (`pipe()` for kernel-console FileHandle backing) |

Drift between this binary and `elastos-vz/Cargo.toml` is a bug. The
whole point of pinning is that what passes here corresponds bit-for-bit
to what `elastos-vz` will see at compile time.
