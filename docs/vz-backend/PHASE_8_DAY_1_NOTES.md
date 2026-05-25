# Phase 8 Day 1 — Rootfs artifact strategy decision (Ubuntu squashfs)

**Phase**: 8 (Real workload on Mac — boot to userspace + run a capsule)
**Day**: 1 (rootfs strategy decision; pure documentation, no code)
**Date**: 2026-05-25
**Status**: GREEN — Phase 8 scope defined; rootfs format + source
decided as `ubuntu-22.04-server-cloudimg-arm64.squashfs` from
Canonical's `release-20260515` pinned path, byte-mounted directly by
the kernel as squashfs (no format conversion needed). The substrate
already wires block devices end-to-end (`elastos-vz/src/ffi/builder.rs:102`
calls `build_block_device(&vm.rootfs_path, ...)` and `:174` calls
`setStorageDevices`); Phase 8 is purely a distribution + integration
exercise, not a substrate exercise.
**Predecessor**: [`PHASE_7_DAY_6_NOTES.md`](./PHASE_7_DAY_6_NOTES.md) — closes Phase 7.
**Successor**: Phase 8 Day 2 — `components.json` rootfs entry + setup fetcher landing.

---

## 1. Headline

The Mac Vz substrate's last gap to "run a real workload" is a
bootable rootfs file on disk at the path the kernel will mount. The
substrate already knows how to attach block devices to the
microVM — what's missing is the artifact.

**Decision**: pin
`https://cloud-images.ubuntu.com/releases/jammy/release-20260515/ubuntu-22.04-server-cloudimg-arm64.squashfs`
(411 MB, SHA256-verified against Canonical's signed `SHA256SUMS`)
as the `darwin-arm64` base rootfs. Save it byte-for-byte to
`<capsules-dir>/ubuntu-base/rootfs.ext4` (filename convention is
just the supervisor's discovery glob — the actual format is
squashfs and the kernel mounts it via `rootfstype=squashfs` in
boot args).

**Why squashfs vs the other options**: it's a read-only Linux-
native filesystem the kernel mounts directly. No format conversion,
no host tooling, no `mkfs.ext4`, no `qemu-img`, no qcow2 crate
dependency — just download, checksum, save. The "read-only" trade-
off is fine for the v0.1 demo (writes go to tmpfs at boot) and
matches the upstream Linux-side `RootfsManager` overlay
pattern where the base is read-only and per-VM writability comes
from overlays.

## 2. Substrate readiness audit (what already works)

The audit done during Day-1 pre-work surfaced that the Mac Vz
substrate is dramatically further along than the day's prompt
implied. Each finding is independently verifiable in the repo:

### 2.1 Block-device FFI is complete and 3/3 tested

[`elastos-vz/src/ffi/block.rs`](../../elastos/crates/elastos-vz/src/ffi/block.rs)
implements `build_block_device(disk_path, read_only)` against
`VZDiskImageStorageDeviceAttachment` with `Cached + Fsync` caching/
sync modes (Lima/UTM production defaults from issue #4840). Three
unit tests cover missing-file rejection, read-write attachment,
and read-only attachment. **No new FFI work needed in Phase 8.**

### 2.2 The launch path already attaches the rootfs

[`elastos-vz/src/ffi/builder.rs:102`](../../elastos/crates/elastos-vz/src/ffi/builder.rs):

```rust
let rootfs = build_block_device(&vm.rootfs_path, vm.rootfs_readonly)
    ...
unsafe { cfg.setStorageDevices(&storage) };  // :174
```

The Vz launch builder already pulls the rootfs path off `VmConfig`,
builds a `VZVirtioBlockDeviceConfiguration`, and wires it into the
Vz machine config. **No new builder work needed.**

### 2.3 `VmConfig` already has rootfs fields, with `from_manifest`

[`elastos-vz/src/config.rs:285,288,302,345,384`](../../elastos/crates/elastos-vz/src/config.rs):

```rust
pub struct VmConfig {
    pub rootfs_path: PathBuf,          // required, not Option
    pub rootfs_readonly: bool,
    pub data_disk_path: Option<PathBuf>,
    ...
}

pub fn from_manifest(...) -> VmConfig {
    // line 384: rootfs_path: capsule_path.join(&manifest.entrypoint),
    ...
}
```

The supervisor's per-capsule `VmConfig` is already constructed
with a rootfs path resolved from the capsule's `entrypoint`
manifest field. **No new VzConfig/VmConfig API surface needed.**

### 2.4 An integration test already expects a real rootfs

[`elastos-vz/tests/concurrent_launch.rs:191–252`](../../elastos/crates/elastos-vz/tests/concurrent_launch.rs):

```rust
fn discover_rootfs() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ELASTOS_VZ_TEST_ROOTFS") { ... }
    let capsules_dir = ~/.local/share/elastos/capsules;
    for entry in std::fs::read_dir(&capsules_dir) {
        let rootfs = entry.path().join("rootfs.ext4");
        if rootfs.is_file() { return Some(rootfs); }
    }
    None
}

#[tokio::test]
async fn concurrent_load_with_real_kernel() {
    ...
    let rootfs = match discover_rootfs() {
        None => { eprintln!("skipping — no rootfs found ... Run `elastos setup` first."); return; }
        ...
    };
    // Boots 3 VMs against this rootfs in parallel.
}
```

The test prints a clear "skip" message when no rootfs is found.
The substrate is *waiting* for someone to ship a rootfs. The Day-1
pre-work hadn't surfaced this — and it changes the Phase 8 shape
from "build a new pipeline" to "land an artifact and watch the
existing pipeline run."

### 2.5 Linux-side `RootfsManager` shows the overlay pattern to mirror

[`elastos-crosvm/src/rootfs.rs:45–73`](../../elastos/crates/elastos-crosvm/src/rootfs.rs):

`get_or_create_overlay(vm_id, base_rootfs)` does a plain
`tokio::fs::copy` of the base rootfs to a per-VM `<vm_id>.ext4`
file in the overlay dir. **Phase 8 will reuse this exact pattern
on Mac** (the existing supervisor-side prune already cleans up
`*.ext4` files in `rootfs_cache_dir/overlays`, per
[`supervisor.rs:198–222`](../../elastos/crates/elastos-server/src/supervisor.rs)).

### 2.6 Net Phase 8 surface

| substrate gap     | reality                                                                |
|-------------------|------------------------------------------------------------------------|
| Block device FFI  | ✅ done in Phase 0-1                                                   |
| Vz launch wires it | ✅ done in Phase 2-3                                                   |
| `VmConfig.rootfs_path` | ✅ done in Phase 3                                                |
| Integration test  | ✅ skip-aware test exists, ready                                       |
| Linux overlay     | ✅ done in Phase 0                                                     |
| **The rootfs file itself** | ❌ **THIS is Phase 8** — distribution only, no new code surface |

## 3. Trade space — three real candidates, evaluated

| Option | Source artifact | Size | Format issue | Host tooling needed | Boot-arg story |
|--------|-----------------|------|--------------|---------------------|----------------|
| **A — squashfs** ✅ chosen | `ubuntu-22.04-server-cloudimg-arm64.squashfs` | 411 MB | none — kernel mounts directly | none | `root=/dev/vda rootfstype=squashfs rw` + tmpfs overlay |
| B — qcow2 + Rust converter | `ubuntu-22.04-server-cloudimg-arm64.img` | 636 MB → ~2-3 GB raw | qcow2 → raw at install time | `qcow2-rs` crate dep | `root=/dev/vda rootfstype=ext4 rw` |
| C — root tarball + mkfs | `ubuntu-22.04-server-cloudimg-arm64-root.tar.xz` | 372 MB | tar.xz extracted → ext4 filesystem must be created | `mkfs.ext4` from e2fsprogs (brew install) + `tar` (preinstalled) | `root=/dev/vda rootfstype=ext4 rw` |
| D — Alpine minirootfs | `alpine-minirootfs-3.19-aarch64.tar.gz` | 4 MB | same ext4 build problem as C | same as C | Alpine + Ubuntu kernel mismatch risk |
| E — Custom-built ext4 | new pipeline producing pre-built ext4 | ~50-200 MB | none (we built it ourselves) | 2-3 days of release work in a separate Phase | full control |

### Why Option A wins for v0.1

1. **Operationally zero-friction**: no host tooling, no Rust crate
   added, no subprocess calls. The Day-2 fetcher reuses the
   existing `download → verify SHA256 → write to install_path`
   path. Same shape as the kernel + initrd lines in
   [`components.json`](../../components.json).
2. **Pinned + signed**: SHA256 in Canonical's
   `release-20260515/SHA256SUMS` (GPG-signed). Same provenance
   chain as our Phase 7 kernel + initrd.
3. **Compatibility-proven**: squashfs has been a stock Linux
   kernel module since 2.6.29 (2009). Ubuntu's `5.15.0-179-generic`
   ships it built-in.
4. **Smaller than qcow2 raw**: a sparse 636 MB qcow2 expands to
   2–3 GB raw, blowing the user's disk budget. 411 MB is a 5×
   reduction.
5. **Matches Linux side conceptually**: Linux side has read-only
   base + writable overlay. We get the same semantic for free
   (squashfs is read-only by definition; tmpfs overlay handles
   writes in RAM).

### Why options B–E are not Day-2 targets

- **B (qcow2 + Rust)**: introduces a new crate dep and ~2 GB on-
  disk footprint. Surveyed crates: `qcow2-rs` (last updated 2024,
  pure-read; would need wrapping in a converter), `qcow2`
  (deprecated). The "no host tooling" benefit is real but the
  storage cost is unacceptable.
- **C (root tarball + mkfs.ext4)**: needs a brew install for
  `mkfs.ext4`. Day-6 just landed clean `elastos doctor` UX — we
  don't want to immediately require operator-side tool installs.
- **D (Alpine)**: musl libc userspace through a glibc-flavored
  initrd. The kernel boots fine, but the initrd's mount hooks
  and runtime expectations may panic. Not the right v0.1 bet.
- **E (custom pipeline)**: 2–3 days of build engineering for
  marginal benefit. Defer to Phase 9+ when we want a 50 MB image
  with only `elastos-runtime` userspace.

## 4. Acceptance bar for v0.1 ("Mac works")

By end of Phase 8 (target: Day 4–6), this user-facing transcript
must work on a clean Mac:

```bash
# One-time setup
$ elastos setup --profile minimal
ElastOS v0.2.0-dev — setup for darwin-arm64
   Downloading vmlinux ... ✓ (15 MB)
   Downloading initrd ...  ✓ (32 MB)
   Downloading rootfs ...  ✓ (411 MB)
3 components installed.

$ elastos doctor
ElastOS doctor — substrate path resolution check
  platform:   darwin-arm64
  data_dir:   /Users/.../Library/Application Support/elastos

  vmlinux:    .../bin/vmlinux                     [present] passes guest-kernel sanity check
  initrd:     .../bin/initrd                      [present]
  rootfs:     .../capsules/ubuntu-base/rootfs.ext4 [present] size 411 MB
  state_dir:  .../vz                              [absent — will be created on first launch]

# The actual demo
$ elastos run ubuntu-base
[vm_console] [    0.123] Linux version 5.15.0-179-generic ...
[vm_console] [    2.500] Mounting / as squashfs (read-only)
[vm_console] [    3.200] Ubuntu cloud-init starting ...
[vm_console] [    4.100] hello from elastos on macOS!
[vm_console] [    4.150] systemd: System has finished booting
elastos run: capsule exited cleanly.
```

Three things are inside the v0.1 bar:
1. **Setup downloads a rootfs alongside kernel + initrd.**
2. **Doctor reports the rootfs as installed.**
3. **`elastos run` boots the rootfs to a real userspace marker
   (Ubuntu's `cloud-init` will produce many lines without us
   doing anything; the "hello" marker is an optional Day-5
   refinement via cloud-init user-data injection).**

Six things are explicitly OUT of the v0.1 bar (deferred to
Phase 9+):
- Network connectivity from the guest to host services.
- Persistent on-disk state across reboots (writes to tmpfs only).
- Passing custom arguments from host to guest.
- Chat capsule, file-share capsule, browser capsule.
- The Linux-side `chat-linux-arm64.tar.gz` bundle publication.
- GPU passthrough / Metal sharing / virtio-gpu.

## 5. Phase 8 day breakdown (4–6 days estimate)

Each day produces one commit + one notes file, same cadence as
Phase 7. Day-by-day, ordered by dependency:

### Day 2 — `components.json` rootfs entry + setup fetcher

- Add `external.rootfs` component to `components.json` (mirror the
  `initrd` entry shape from Phase 7 Day 2).
- `darwin-arm64` platform entry: URL = Canonical's squashfs URL,
  SHA256 from `SHA256SUMS`, size 411 MB, `compression: null`
  (squashfs is not double-compressed — the file IS the on-disk
  format).
- Fetcher work: minimal — squashfs is byte-saved, same as initrd.
  The only schema concern is *where* to install it: the capsule
  convention is `<capsules_dir>/<name>/rootfs.ext4`, not
  `<data_dir>/bin/rootfs`. The fetcher needs to land it under
  `~/Library/Application Support/elastos/capsules/ubuntu-base/rootfs.ext4`.
- Update profiles: `minimal/chat/full` profiles include `rootfs`.
- Expected: 1 unit test + manual `elastos setup` smoke.

**Risk**: capsules currently expect a `manifest.json` alongside
the rootfs (`from_manifest` reads it). Day 2 likely also needs
to ship a stub capsule manifest for `ubuntu-base`. If so, that's
a small JSON file in the components install path.

### Day 3 — Supervisor + doctor wiring

- `elastos doctor` should report the rootfs row alongside vmlinux/
  initrd. Add a `rootfs` row to `print_report` and a `rootfs`
  component lookup (small, ~10 LOC).
- Verify the supervisor's existing capsule auto-discovery picks
  up `ubuntu-base` once installed (no code change expected —
  validation only).
- 1 new doctor test + 1 new supervisor test for the install-path
  resolution.

### Day 4 — Run the existing `concurrent_load_with_real_kernel` test

- This test already exists and is skip-on-no-rootfs. With Day-2
  artifacts staged, it should:
  1. Discover the kernel.
  2. Discover the rootfs (Day-2 staged it).
  3. Boot 3 parallel VMs.
- Expected outcomes:
  - **Best case**: all 3 boot, validate-with-error passes, the
    test moves through. Phase 8 is effectively done — the user-
    facing UX is now just `elastos run` plumbing.
  - **Likely case**: boot fails on kernel cmdline (we haven't
    set `root=/dev/vda rootfstype=squashfs` yet). Day-4 then
    adds the boot-arg adjustment to `VmConfig::from_manifest`
    for the squashfs path.
  - **Worst case**: Ubuntu's initrd expects a writable rootfs
    (cloud-init writes to `/etc/machine-id` early). If so, Day-4
    extends the overlay path or switches to a writable per-VM
    tmpfs over the squashfs base.

### Day 5 — `elastos run ubuntu-base` end-to-end CLI smoke

- Wire the CLI dispatch path: `elastos run <capsule>` should
  resolve `ubuntu-base` through the supervisor, get the capsule
  loaded, and stream console output to the user's terminal via
  `vm_console` tracing.
- If the existing `elastos run` path already does this on Linux
  (it does — see `main.rs:1058` `Commands::Run`), the work is
  just verifying the Mac code path doesn't break.
- 1 manual smoke test.

### Day 6 — Phase 8 closing notes

- Aggregate all Day-2/3/4/5 findings into a Phase-8 close doc.
- Update [`PHASE_6_PLAN.md`](./PHASE_6_PLAN.md) banner to say
  "Mac runs real Linux userspace through ElastOS — substrate +
  distribution complete."
- Decide on Phase 9 scope (network, persistent storage, chat
  capsule, browser, etc.).

### Compression possibility

If Days 2–4 go quickly, Day-5 + Day-6 can compress into a single
day; Phase 8 could close in 4 days instead of 6. Day-3 is the
biggest variable — if doctor wiring is trivial it's hours, not
a day.

## 6. What this explicitly is NOT (scope boundary)

Day-1 is a decision day. It does NOT:

- Edit `components.json`.
- Edit any Rust code.
- Add any new crate dependencies.
- Add or remove tests.
- Run `elastos setup` or any other CLI command.
- Touch the substrate (`elastos-vz`, `elastos-crosvm`).

The only deliverables today are:
- This decision document.
- A one-line forward-link in [`PHASE_6_PLAN.md`](./PHASE_6_PLAN.md).
- A commit on `sash/local-test`.

If during Day-2 a fact in this document turns out to be wrong
(e.g. Ubuntu's squashfs doesn't boot cleanly under our pinned
kernel), Day-2's notes will document the pivot and the trade
space gets revisited. That's the same model Phase 7 Day 1 used.

## 7. Open questions deferred to Day 2+

These are real questions but answering them today would over-scope
Day 1. Each is tagged with the Day where it gets resolved:

1. **Capsule manifest shape for `ubuntu-base`**: does the supervisor
   require a `manifest.json` alongside `rootfs.ext4`, or can the
   rootfs stand alone? *Day 2.*
2. **Boot args for squashfs**: what does `VmConfig::from_manifest`
   need to emit to mount the rootfs as squashfs? *Day 4.*
3. **Writable overlay strategy**: tmpfs (simplest), per-VM
   squashfs+ext4 overlay (more complex), or kernel `rw` (UFS-
   style writable squashfs)? *Day 4.*
4. **`rootfs.ext4` naming**: do we rename the supervisor's
   discovery glob to be format-agnostic, or just store the
   squashfs bytes under the existing `.ext4` filename
   convention (treating the extension as a stable identifier
   not a format claim)? *Day 2.*

## 8. Files changed (full inventory)

| file                                                              | delta            | role                                                    |
|-------------------------------------------------------------------|------------------|---------------------------------------------------------|
| `docs/vz-backend/PHASE_8_DAY_1_NOTES.md`                          | +new (this file) | Phase 8 Day 1 decision document                         |
| `docs/vz-backend/PHASE_6_PLAN.md`                                 | +1 status banner | Phase 7 closed, Phase 8 started                         |

Net: 0 LOC of Rust, 0 LOC of JSON, 0 binary changes. Pure
documentation commit, by design.
