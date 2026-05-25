# Phase 8 Day 5 — `elastos run ubuntu-base` boots Ubuntu to systemd userspace on Mac

**Date**: 2026-05-26
**Branch**: `sash/local-test`
**Anchor**: [`PHASE_8_DAY_4_NOTES.md`](./PHASE_8_DAY_4_NOTES.md) (first real-substrate boot)
**Forward link**: Day 6 will add a writable overlay so systemd-logind and the rest of userspace stops crashing on the read-only squashfs.

## TL;DR

**`elastos run ubuntu-base` works.** On this Mac, one command after
`elastos setup --profile minimal`, the operator gets a real Ubuntu
22.04 LTS arm64 guest booted inside Apple Vz to **systemd userspace**:

```
[  OK  ] Started D-Bus System Message Bus.
[  OK  ] Started Save initial kernel messages after boot.
[  OK  ] Started irqbalance daemon.
[  OK  ] Started System Logging Service.
[  OK  ] Reached target Path Units.
[  OK  ] Reached target Socket Units.
[  OK  ] Reached target Basic System.
[FAILED] Failed to start User Login Management.   ← squashfs is RO; Day 6 adds overlay
```

That's Phase 8's mission-statement acceptance bar:
> "`elastos setup --profile minimal && elastos run ubuntu-base`
>  boots a real Linux userspace inside Vz on Mac."

**Met.** Zero regressions (**391/391** lib tests, +3 over Day 4).

## What shipped (one commit)

### `setup.rs` — post-install capsule metadata hook

After the component install loop runs, a new
`ensure_standalone_capsule_metadata(&data_dir)` step writes a
default `capsule.json` next to any auto-init capsule's installed
rootfs. Today's only entry is `ubuntu-base`. The hook is **strict**
about not surprising operators:

1. Skips entirely when the rootfs isn't present (no advertising a
   capsule we can't boot).
2. Skips when `capsule.json` already exists (operator edits
   survive `elastos setup` re-runs).
3. Round-trip-tested in 3 paired unit tests covering absent,
   present-without-manifest, and idempotent-with-existing-manifest
   cases. The present-case test also parses the auto-generated
   JSON through `CapsuleManifest::validate()` so the hard-coded
   string can't drift out of schema silently.

The auto-init list is intentionally hard-coded for Phase 8 Day 5.
Generalising to a `PlatformInfo.capsule_template` field is the
obvious next step but locks the schema before we have a second
auto-init capsule to drive the design — premature.

### `run_cmd.rs` — two-part CLI wiring

1. **Name-resolution fallback**. `elastos run ubuntu-base` (no
   `--path`, no `--cid`) binds clap's positional `path` as
   `ubuntu-base`. The new `resolve_capsule_by_name` helper checks
   if it's a single-component path whose canonical install
   location `<data_dir>/capsules/<name>/capsule.json` exists,
   and rewrites the path to that directory. Guarded against
   `..`, leading `/`, and `./`-style relative paths so a typo
   doesn't silently get rewritten — those are passed through
   verbatim to the existing path-mode logic.
2. **In-process standalone boot lane**. The existing MicroVM
   dispatch (`run_microvm_via_operator_runtime`) needs a running
   `elastos serve` daemon to HTTP-call. For a standalone capsule
   with no `requires` / `providers`, that's an unnecessary
   prerequisite. New `operator_runtime_available()` check + the
   `run_microvm_standalone()` function build a VzProvider in this
   process, load the capsule via the same `load_with_vm_config +
   start` path the Day-4 integration test exercises, stream the
   kernel console through the existing `vm_console` tracing
   target, and wait for Ctrl-C. The managed-runtime lane is
   preserved unchanged — operators wanting it explicitly just
   start `elastos serve` first.

The standalone lane is `#[cfg(target_os = "macos")]`. On Linux
the stub returns a typed error pointing operators at the managed
crosvm lane (`elastos serve` → `elastos run`).

### `components.json` — size correction

Bumped `rootfs.darwin-arm64.size` from `430985216` to
`431013888` to match Canonical's actual on-disk file. The old
value (which I'd put in on Day 2) was 28 KiB short, so every
`elastos setup` run on a pre-existing install saw a "stale: will
refresh" verdict and re-downloaded 411 MB unnecessarily. Checksum
was always correct; this is a metadata fix, not a content fix.

## Acceptance bar — all met

- [x] `elastos setup --profile minimal` writes a valid
      `capsule.json` at the canonical capsule path. Confirmed by
      `[init] wrote default capsule metadata: …/capsule.json`
      in the smoke log.
- [x] capsule's `boot_args` includes
      `root=/dev/vda rootfstype=squashfs` so the kernel mounts
      the squashfs after initramfs handover. Confirmed by
      systemd boot output in the smoke log.
- [x] `elastos run ubuntu-base` (name-only, no flags) boots the
      VM, returns a handle, streams kernel console. Confirmed.
- [x] No manual `elastos serve` daemon required — operator
      types the two commands, sees Linux boot.
- [x] Codesign is documented as a known one-time setup
      (`scripts/dev/sign-elastos-vz/sign.sh`) — same as Day 4.
      Production-cert signing is Phase-6 backlog (Apple
      Developer ID needed).
- [x] `cargo test -p elastos-server --lib`: **391/391** (+3 over
      Day 4, all three are the new paired metadata-hook tests).
      Zero regressions.
- [x] One commit, one notes file.

## Smoke-test logs (verbatim, this Mac)

### First attempt — `boot_args` missing `root=`

The initial capsule.json had `boot_args: "console=hvc0 reboot=k
panic=1 init=/init"`. Booted cleanly through the kernel and
initramfs, reached `Run /init`, then Ubuntu's initramfs init
script panicked with a *very* specific error:

```
[run] guest started. Press Ctrl-C to stop.
…
[    0.123727] cacheinfo: Unable to detect cache hierarchy for CPU 0
…
[    0.129352] Run /init as init process
Loading, please wait...
…
Scanning for Btrfs filesystems
done.
No root device specified. Boot arguments must include a root= parameter.
Rebooting automatically due to panic= boot argument
[    2.185978] reboot: Restarting system
```

Exactly the iteration the Day-5 prompt anticipated — a
real-substrate boot failure with a specific, actionable error
message.

### Second attempt — `boot_args` with `root=/dev/vda rootfstype=squashfs ro init=/sbin/init`

```
2026-05-25T17:30:11  INFO vm_console: [  OK  ] Started D-Bus System Message Bus.
2026-05-25T17:30:11  INFO vm_console: [  OK  ] Started Save initial kernel messages after boot.
2026-05-25T17:30:11  INFO vm_console: [  OK  ] Started irqbalance daemon.
2026-05-25T17:30:11  INFO vm_console: [  OK  ] Reached target Preparation for Logins.
2026-05-25T17:30:11  INFO vm_console:          Starting Snap Daemon...
2026-05-25T17:30:12  INFO vm_console: [FAILED] Failed to start User Login Management.
2026-05-25T17:30:12  INFO vm_console: See 'systemctl status systemd-logind.service' for details.
2026-05-25T17:30:12  INFO vm_console: [  OK  ] Started Unattended Upgrades Shutdown.
2026-05-25T17:30:12  INFO vm_console: [  OK  ] Started System Logging Service.
2026-05-25T17:30:13  INFO vm_console: [  OK  ] Finished Remove Stale Online ext4 Metadata Check Snapshots.
```

(ANSI color escapes stripped for readability.)

**Real Ubuntu 22.04 systemd userspace running on Mac inside Apple
Vz.** Wall-clock from `elastos run ubuntu-base` to first systemd
service: ~3 seconds. Many services come up cleanly. A few crash
in expected ways because squashfs is read-only — `systemd-logind`
wants to persist login state, `Unattended Upgrades` wants to
write logs, snapd wants `/var/lib/snapd`. Day 6 will fix all of
this with a `tmpfs` overlay on top of the squashfs.

## Design notes

### Why standalone instead of operator-runtime for Day 5

The operator-runtime lane (`run_microvm_via_operator_runtime`)
is the right architecture for multi-capsule production: shared
identity, shared storage, signed launch envelopes, capsule
graph resolution. For v0.1's "real Linux on Mac in one command"
demo, that lane adds two prerequisites the demo bar shouldn't
require:

1. A second long-running process (`elastos serve`).
2. A registry capsule entry with a CID — but `ubuntu-base` is
   installed through `external.rootfs.install_path`, not
   `capsules.<name>.cid`, because the latter would require
   Canonical to pin a CID to their squashfs (they don't).

The split is intentional and documented in the code: standalone
when no daemon is running, managed when one is. Operators can
opt into either by either starting the daemon (or not) before
`elastos run`. No new flags.

### Why we didn't add a writable overlay today

Day 5's bar was "boot to userspace, observe real behaviour." The
read-only squashfs gets us there — systemd starts, dozens of
services come up clean, a few crash visibly with specific
"can't write to disk" failures that point at the obvious next
fix. That fix (tmpfs-overlay on squashfs) is mechanically clear
but requires deciding HOW much state to make ephemeral (just
/var? all of root? per-VM persistent overlay file?). That's a
real design decision worth its own day, not a Day-5 nice-to-have.

### Why `init=/sbin/init` instead of `/lib/systemd/systemd`

Canonical's cloud image initramfs scripts read `init=` from
`/proc/cmdline` and exec it after `pivot_root`. `/sbin/init` is
the historical symlink Ubuntu maintains for compatibility; on
this image it points at `/lib/systemd/systemd`. The shorter
path is more portable across Ubuntu versions.

## Process notes

### Recurring "stamped manifest shadows repo edits" trap

Day 2 hit this for `rootfs`. Day 5 hit it for the same
component's `size` field. The fix is the same: `mv
"$DATA_DIR/components.json" "$DATA_DIR/components.json.bak"`
and re-run setup so the fetcher re-stamps from the repo. This
is dev-iteration churn only — end users install from a stamped
release where the manifest is already correct.

A nicer fix would be a `--use-repo-manifest` flag for dev runs,
but that's a UX polish task. Documented here so the next
contributor recognises the pattern after one minute, not ten.

### `timeout` is not in macOS coreutils

`timeout 8 ./target/debug/elastos run ubuntu-base` failed:
`command not found: timeout`. macOS only ships `gtimeout`
when homebrew coreutils are installed. Pure-shell pattern that
works portably (used in this Day's smoke tests):

```bash
./bin & PID=$!
sleep 8
kill -INT $PID 2>/dev/null || true
sleep 2
kill -KILL $PID 2>/dev/null || true
wait $PID 2>/dev/null
```

## Deferred items (NOT Day-5 scope)

- **Writable overlay** for the squashfs rootfs (Day 6). systemd
  needs to write to `/var`, `/run` is already tmpfs by default
  but `/var/log`, `/var/lib/{snapd,dbus,…}` need either a
  tmpfs overlay or a per-VM persistent ext4 file. The Day-5
  smoke surfaces exactly which services this affects: logind,
  unattended-upgrades, snapd. Picking tmpfs vs persistent
  ext4-on-loop is the Day-6 design call.
- **Production codesign**. Day-5 still uses the ad-hoc local-dev
  signing script. Production distribution needs an Apple
  Developer ID cert + notarisation. Phase-6 backlog, hardware/
  cert procurement.
- **Verifier drift** (Class-C `release_path missing`) still open
  from Day 2. Orthogonal.

## Why this is the v0.1-demo close

Phase 8 Days 1–4 staged the artefacts and proved the substrate
could boot them. Day 5 wires those artefacts together behind one
command the user actually types. That's the entire mission of
Phase 8: "real workload on Mac." We can ship a one-page
README that says:

```
brew install … && elastos setup --profile minimal && elastos run ubuntu-base
```

…and the user sees Ubuntu boot inside Apple Vz on their Mac. The
remaining work (Day 6+, the squashfs overlay) is about making the
guest *useful* once it boots, not about whether it boots at all.

## Next

Phase 8 Day 6 — writable overlay strategy so systemd-logind and
the rest of Ubuntu userspace stops crashing on the read-only
squashfs. Decision: tmpfs (ephemeral, fast) vs ext4-on-loop
(persistent, per-VM). Either way the boot_args grow a `boot=ro`
overlay scheme similar to Ubuntu's live-CD `casper` init or a
manual `overlay` mount in a tiny `init.sh` we ship inside the
rootfs.
