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
| `type: microvm` (e.g. `chat`, `agent`, `localhost-provider`, `did-provider`, `shell`, `webspace-provider`, `ipfs-provider`, `tunnel-provider`, ...) | KVM + crosvm on Linux; Apple Vz on macOS *(in progress — see below)* | **Day-5 boot proved a real Linux guest boots under Vz.** Phase 3 Day 1 shipped the supervisor → `VzProvider` seam; Day 2 ported the full substrate-agnostic launch prefix; Day 3 added `CapsuleBackend::VzVm` so `elastos ps`, `elastos status <handle>`, and `elastos stop <handle>` work on Mac. **Phase 3 Day 4 replaced the placeholder Carrier console with a real `socketpair(AF_UNIX, SOCK_STREAM)`** — bytes now flow guest↔host on `/dev/hvc1`. Capsule code inside the VM can talk to host providers via the same `RequestEnvelope` / `ResponseEnvelope` protocol the Linux flow uses; first-party capsules (`chat`, `did-provider`, …) are launchable end-to-end on Mac. vsock host→guest bridging and `VZVirtualMachineDelegate`-driven exit codes remain Day 5; TAP networking is rejected with a typed entitlement-required message (no silent NAT downgrade). See [`vz-backend/PHASE_3_DAY_4_NOTES.md`](vz-backend/PHASE_3_DAY_4_NOTES.md). |

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
