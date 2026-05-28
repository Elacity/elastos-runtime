# Phase 8 Day 6 — writable tmpfs overlay; Ubuntu boots to `login:` on Mac

**Date**: 2026-05-26
**Branch**: `sash/local-test`
**Anchor**: [`PHASE_8_DAY_5_NOTES.md`](./PHASE_8_DAY_5_NOTES.md) (booted to systemd userspace with 10+ FAILED services)
**Forward link**: Day-7 candidates are (a) persistent overlay variant behind a capsule manifest flag, (b) input/output console attach for interactive shell, (c) cloud-init seed for hostname/SSH-keys.

## TL;DR

The Day-5 boot stalled because Canonical's squashfs is read-only and Ubuntu
userspace (`systemd-logind`, `rsyslog`, `unattended-upgrades`, …) crashes
on every write attempt. Day 6 fixes that with a **tmpfs overlay**:

- **`overlay_initrd.rs`** (new module): minimal newc-format CPIO
  writer + a single `/init` script that mounts the squashfs at
  `/lower`, a tmpfs at `/upper` (256 MiB ephemeral), overlays them
  at `/newroot`, then `switch_root`s into the merged tree and execs
  `/sbin/init`. The CPIO is appended to Canonical's pristine
  `bin/initrd` at `elastos setup` time (idempotent byte-compare).
- **Three consumers** (supervisor, `elastos run` standalone lane,
  test discovery) all go through a shared `resolve_initrd_path()`
  helper, so the overlay variant wins automatically when present
  and the pristine variant is the fallback.

### Before / after on this Mac

| Metric                        | Day 5         | Day 6         |
|-------------------------------|---------------|---------------|
| Boot reaches `Run /init`      | ✅            | ✅            |
| Squashfs mounts as rootfs     | ✅            | ✅            |
| systemd reaches userspace     | ✅            | ✅            |
| **`[FAILED]` services**       | **~10**       | **1**         |
| `[  OK  ]` services           | ~40           | **110**       |
| `Started User Login Management` | **never**   | ✅            |
| `Started System Logging Service` | only late w/ multiple retries | ✅ once |
| `Started OpenBSD Secure Shell server` | never  | ✅            |
| `Reached target Login Prompts` | never        | ✅            |
| `ubuntu login:` on the screen | **never**     | ✅            |

The single remaining `[FAILED]` is
**Device-Mapper Multipath Device Controller** — `multipath-tools`,
which is for SAN/iSCSI multipathing. It's expected to fail in
*any* microVM (or any host without SCSI multipathing) and is
unrelated to our overlay. We could remove it from the cloud
image's preset but it's a single benign systemd unit; not worth
the scope expansion.

**Headline: `elastos run ubuntu-base` brings up a fully booted
Ubuntu 22.04.5 LTS with a login prompt on a Mac, from one
command after `elastos setup`.**

Zero regressions: **403/403** elastos-server lib tests
(+12 over Day 5 — 9 CPIO tests, 3 resolver tests).

## What shipped (one commit)

### `overlay_initrd.rs` — newc-CPIO writer + `/init` injector

A self-contained ~200-line module that:

1. Builds a minimal Linux newc-CPIO archive in memory. Pure
   Rust — no `mkinitramfs`, no `cpio` shell-out, no vendoring of
   Canonical's initramfs build chain. The format spec is in
   `init/initramfs.c` in the Linux kernel; one CPIO entry per
   file, 110-byte header + 4-byte-aligned name + 4-byte-aligned
   payload, terminator named `TRAILER!!!`.
2. Ships a single CPIO entry: `/init` (mode 0100755) with our
   custom shell script.
3. `write_combined_initrd(source, dest)` reads Canonical's
   pristine initrd, byte-appends our CPIO, writes the result.
   Idempotent — byte-compares against any existing destination
   and short-circuits when content matches.

The CPIO writer is tested 9 ways:

- Magic byte at offset 0 (`070701`).
- Filename + script payload literals appear in the body.
- `TRAILER!!!` sentinel present.
- 4-byte alignment of the archive end.
- **Round-trip through the system `cpio` parser** (`cpio -t -F`
  must list `init`). This is the gold-standard binary
  compatibility check — same parser the Linux kernel models.
- `write_combined_initrd`: appends correctly, idempotent,
  rewrites when source changes.
- Mode constant matches Unix `0100755`.

### The `/init` script

The kernel concatenates initramfs segments. Ubuntu's pristine
initrd unpacks first (loads `/bin/sh`, `/usr/bin/mount`,
`/usr/bin/switch_root`, `kmod`, the `overlay.ko` module, etc.).
Our segment unpacks next; its `/init` shadows Ubuntu's. So our
script can use Ubuntu's tools — we don't ship binaries, only the
script.

The script is intentionally minimal:

```sh
mount -t proc proc /proc; mount -t sysfs sys /sys; mount -t devtmpfs dev /dev
modprobe overlay; modprobe virtio_blk; modprobe squashfs

# Parse root=, rootfstype=, init= from /proc/cmdline.
# Defaults: /dev/vda, squashfs, /sbin/init.

# Wait up to 5s for the block device to appear (virtio PCI
# probing is async; first attempt without this lost ~100% of
# the time — see § "Iteration log" below).

mount -t squashfs -o ro $ROOT /lower
mount -t tmpfs    -o size=256m tmpfs /upper
mkdir /upper/upper /upper/work
mount -t overlay overlay \
    -o lowerdir=/lower,upperdir=/upper/upper,workdir=/upper/work \
    /newroot

mount --move /proc /newroot/proc
mount --move /sys  /newroot/sys
mount --move /dev  /newroot/dev

exec switch_root /newroot $INIT_CMD
```

Plus a debug-fallback branch: if `/dev/vda` doesn't appear after
5s, we print `/dev` listing and exec `/sbin/init` from the
initramfs (so the operator sees `which executable failed` rather
than a silent kernel panic).

### Shared resolver: `resolve_initrd_path(bin_dir) -> Option<PathBuf>`

Three consumers used to hard-code `bin/initrd`. Now they all go
through one helper that prefers `bin/initrd-overlay` when
present, falls back to `bin/initrd`, returns `None` when
neither is staged. The resolver lives in `overlay_initrd.rs`
alongside the writer so the fallback chain is colocated with the
artifact it produces.

Consumers updated:

- `supervisor.rs` (Mac branch of `Supervisor::new_with_vz_config`).
- `run_cmd.rs` (standalone in-process MicroVM lane).
- `elastos-vz/tests/concurrent_launch.rs` (integration-test
  artifact discovery).

3 paired resolver tests guard the precedence (overlay wins,
pristine fallback, `None` on empty install).

### `setup.rs` — post-install hook

After the existing `ensure_standalone_capsule_metadata` step,
`ensure_overlay_initrd(&data_dir)` runs. No-ops when
`bin/initrd` isn't present (kernel-only boot paths preserved).
Idempotent on every subsequent `elastos setup` run.

## Acceptance bar — met

- [x] Zero `[FAILED]` from write-needing services. Day 5:
      `systemd-logind`, `unattended-upgrades`, `apport`,
      `pollinate`, etc. all crashed. Day 6: all green.
- [x] `Reached target Login Prompts` + `ubuntu login:` prompt
      visible on the captured console.
- [x] tmpfs-overlay strategy picked + documented; ext4-on-loop
      rejected for v0.1 (no persistent state to manage, simpler
      setup story, lower memory floor).
- [x] Idempotent overlay-initrd build (byte-compare short-circuit;
      paired test).
- [x] Three consumers go through one resolver (no
      copy-pasted prefer-overlay logic).
- [x] `cargo test -p elastos-server --lib`: **403/403**
      (+12 over Day 5 — 9 CPIO tests, 3 resolver tests).
      Zero regressions.
- [x] One commit, one notes file.

## Smoke logs (verbatim, this Mac)

### First attempt — `/init` ran but raced virtio probing

```
[    0.141808] Run /init as init process
[    0.146317] /dev/vda: Can't open blockdev
mount: mounting /dev/vda on /lower failed: No such file or directory
[    0.146448] Kernel panic - not syncing: Attempted to kill init! exitcode=0x0000ff00
```

Our `/init` *did* run (CPIO append worked correctly — kernel
unpacked it, shadow-replaced Ubuntu's `/init`), but it tried to
`mount /dev/vda` before the virtio PCI bus had finished
enumerating block devices. Real-substrate bug surfaced; iteration
followed.

### Second attempt — wait loop added

```
[  OK  ] Started D-Bus System Message Bus.
[  OK  ] Started irqbalance daemon.
[  OK  ] Reached target Preparation for Logins.
[  OK  ] Started Save initial kernel messages after boot.
[  OK  ] Started OpenBSD Secure Shell server.
[  OK  ] Reached target Login Prompts.
[  OK  ] Started User Login Management.
[  OK  ] Started Unattended Upgrades Shutdown.
[  OK  ] Started System Logging Service.
[  OK  ] Started LSB: automatic crash report generation.
[  OK  ] Finished Remove Stale Online ext4 Metadata Check Snapshots.
[  OK  ] Started Dispatcher daemon for systemd-networkd.
[  OK  ] Finished Pollinate to seed the system pseudo random number generator.
Ubuntu 22.04.5 LTS ubuntu hvc0

ubuntu login:
```

Wall-clock from `elastos run ubuntu-base` to `ubuntu login:`:
~6 seconds.

110 [  OK  ] services, 1 [FAILED] (the benign `multipath-tools`
SAN unit). Zero "elastos-init:" markers in the log — the
wait-for-vda branch wasn't triggered, meaning `/dev/vda`
appeared within the 5s window on the first poll cycle (~100ms).

## Iteration log

### Iteration 1: race condition on `/dev/vda`

**Symptom**: `Can't open blockdev`, kernel panic.
**Root cause**: virtio PCI bus enumeration is asynchronous;
our `/init` runs immediately after `unpack_to_rootfs`, well
before the kernel finishes probing the block device. Ubuntu's
pristine `/init` waits for udev to settle before mounting; we
skipped that.
**Fix**: 5-second poll-loop on the existence of the root device
node (`[ -b "$ROOT" ]`), 100ms granularity (50 attempts × 100ms).
Generous ceiling — Vz typically presents `/dev/vda` within ~50ms
of devtmpfs being mounted.
**Fallback**: if the device still doesn't appear after 5s, log
the failure mode + listing of `/dev` + exec `/sbin/init` from
the initramfs (so the operator gets at least one shell prompt
instead of a kernel panic). Defensive only; never triggered in
the successful smoke.

### Iteration 2: nothing — second attempt passed end-to-end

Booted to `ubuntu login:` on the first re-run after the wait
loop landed.

## Design decisions

### Why tmpfs (chosen) instead of ext4-on-loop

| Concern               | tmpfs                          | ext4-on-loop                            |
|-----------------------|--------------------------------|-----------------------------------------|
| Persistent state      | No (ephemeral per VM)          | Yes                                     |
| Setup-time disk file  | None                           | Per-VM sparse file (~100 MB)            |
| Memory cost           | RSS grows w/ writes (256 MB cap) | None (disk-backed)                    |
| First-write latency   | ~µs                            | ms (loop-back overhead)                 |
| Setup complexity      | Zero new code                  | Pre-create file + wire data_disk_path   |
| v0.1 demo fit         | **Strong**                     | Moderate                                |

For v0.1's "Ubuntu boots cleanly" bar, tmpfs is the right
hammer. Persistent state is a real Day-7+ design question
(where does the file live? how do operators size it?
does it migrate across `elastos setup` upgrades?) — answered
once we have a workload that actually wants persistence.

### Why CPIO concatenation instead of replacing the initrd

| Concern                       | Concat (chosen)         | Replace                          |
|-------------------------------|-------------------------|----------------------------------|
| Initramfs binaries (sh, mount)| Inherited from Ubuntu   | Must ship ourselves              |
| Kernel module .ko files       | Inherited from Ubuntu   | Must vendor (kernel-version-specific) |
| Build dependencies            | None                    | `mkinitramfs` + Linux toolchain  |
| Vendor drift risk             | Zero                    | High (each kernel bump rebuilds) |
| Binary size                   | +2 KB                   | ~31 MB (everything)              |
| Implementation                | ~200 LOC pure Rust      | Substantial vendoring effort     |

Concatenation is the obvious winner. The Linux kernel
explicitly supports multiple concatenated initramfs archives
(`init/initramfs.c`) — we're using a documented kernel
feature, not a hack.

### Why a shared resolver function

Three consumers needed the same "prefer overlay, fallback to
pristine" logic: supervisor, standalone-lane `elastos run`,
test discovery. Without the helper, the prefer-overlay branch
would have been copy-pasted three times. The helper is six
lines + three paired tests; the copy-paste cost would have
been higher than the abstraction cost.

## Deferred items

- **Persistent overlay** behind a capsule manifest flag (Day 7+).
  Today's tmpfs loses all writes on VM stop. Some workloads
  (databases, snapd state, ssh host keys) want persistence
  across runs. The wiring is: a new
  `microvm.persistent_overlay: bool` field on `CapsuleManifest`,
  consumed by setup (to pre-create the ext4-on-loop file) and
  by `/init` (to mount it as upperdir instead of tmpfs).
- **Interactive console attach**. `Serial Getty on hvc0` is
  started but the operator can't actually log in: we capture
  the console as tracing events, not as a PTY hand-off. Day-7
  candidate: wire `enable_host_raw_mode_pub()` (used by the
  WASM lane) into the standalone MicroVM path.
- **Cloud-init seed**. Ubuntu's image expects a NoCloud
  datasource on first boot for hostname / SSH keys / user
  setup. Today the boot is silent on these. A tiny FAT-formatted
  `cidata.iso` (attached as `/dev/vdb`) would let the operator
  pre-seed `user-data` + `meta-data`.
- **Apport / multipath** — the one remaining `[FAILED]` is
  cosmetic. We could mask the unit at install time
  (`systemctl mask multipath-tools.service` baked into the
  overlay-init script) but a single benign failure isn't worth
  the scope.

## Process notes

### Why iteration was so fast

Day-5's CLI wiring (`elastos run ubuntu-base` as a standalone
lane) made the iteration cycle ~30 seconds wall-clock:
`cargo build && sign && setup && run`. The `kernel panic →
fix /init → retry` loop happened twice (Iteration 1 → 2)
without ever leaving the chat.

This is the dividend Phase-8-Days-1-5 paid for. Future Day-N
work compounds on the same loop.

### Stale stamped manifest still re-downloads rootfs

Same dev-iteration trap as Days 2 and 5: the stamped
`components.json` in `data_dir` lags the repo's. Until that's
fixed at the fetcher layer, dev iteration on `elastos setup`
needlessly re-downloads the 411 MB rootfs whenever the size or
checksum field changes. End users (one-shot installs from a
release artifact) aren't affected.
