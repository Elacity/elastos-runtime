# Phase 8 Day 2 — `components.json` rootfs entry + setup fetcher install

**Date**: 2026-05-25
**Branch**: `sash/local-test`
**Anchor**: [`PHASE_8_DAY_1_NOTES.md`](./PHASE_8_DAY_1_NOTES.md) (artifact decision)
**Forward link**: Day 3 will wire `elastos doctor` to surface a `rootfs` row.

## TL;DR

`elastos setup --profile minimal` on this Mac now downloads, verifies, and
installs Canonical's pinned Ubuntu 22.04 arm64 squashfs (411 MB) at
`~/Library/Application Support/elastos/capsules/ubuntu-base/rootfs.ext4`.

The `concurrent_load_with_real_kernel` integration test in
`elastos-vz/tests/concurrent_launch.rs` — the one that's been
`#[ignore]`'d on Mac and waiting for a rootfs since Phase 6 — can
now find a rootfs via its capsule-discovery glob.

Zero regressions: **386/386** lib tests pass.

## What shipped (one commit)

### `components.json` — new entry + profile inclusion

- Added `external.rootfs` (mirrors `external.initrd` shape from
  Phase 7 Day 2):
  - `install_path`: `capsules/ubuntu-base/rootfs.ext4`
  - `darwin-arm64`:
    - `url`:
      `https://cloud-images.ubuntu.com/releases/jammy/release-20260515/ubuntu-22.04-server-cloudimg-arm64.squashfs`
    - `checksum`:
      `sha256:d2c9bcc0815a02f09293d56c91b0a8f5b16878977bb38a3b2c1b28efd7d1c1fb`
      (pinned from Canonical's signed `release-20260515/SHA256SUMS`)
    - `size`: `430985216` bytes (411 MB)
- Added `rootfs` to the `minimal`, `chat`, and `full` profiles.
  Pattern matches `initrd`: linux profiles see the name but the
  fetcher silently skips it (no `linux-*` platform entry, so no URL
  resolves and the install loop logs
  `[skip] rootfs — no download URL or CID for linux-…`).

### No code changes required

Pre-work surfaced three things the existing fetcher already handles:

1. `setup.rs:1367` — `download_component` does
   `fs::create_dir_all(parent)` before writing, so the previously
   non-existent `capsules/ubuntu-base/` directory is auto-created.
   No fetcher patch needed.
2. `setup.rs:267` — install destination is just
   `data_dir.join(install_path)`. Any relative path under data_dir
   works; there's nothing `bin/`-specific in the install plumbing.
3. The supervisor's `ensure_capsule` (supervisor.rs:1485-1500)
   gates on `capsule_dir.join("capsule.json").is_file()`, **not**
   `manifest.json`. **For Day 2 we don't need to ship a stub** —
   the integration test the rootfs unblocks
   (`concurrent_load_with_real_kernel`) uses direct glob
   discovery in `tests/concurrent_launch.rs`, which only needs the
   `.ext4` file. Wiring `elastos run ubuntu-base` end-to-end is a
   later-Phase-8 task and will ship its own `capsule.json` then.

## Acceptance bar — all met

- [x] `external.rootfs` entry with darwin-arm64 platform info.
- [x] `minimal` / `chat` / `full` profiles include `rootfs`.
- [x] `elastos setup --profile minimal` downloads → verifies →
      installs at the canonical path.
- [x] `file rootfs.ext4` confirms `Squashfs filesystem, version 4.0,
      xz compressed, 431012242 bytes, 43314 inodes` (matches
      Canonical's published metadata; SHA256 verified pre-write).
- [x] `cargo test -p elastos-server --lib`: **386 passed, 0 failed,
      2 ignored** — zero regressions from the JSON-only change.
- [x] One commit.

## Smoke-test log (verbatim, this Mac)

```text
$ ./target/debug/elastos setup --profile minimal
ElastOS v0.2.0-dev — setup for darwin-arm64
Components to install:
  - crosvm
  - vmlinux [already installed]
  - initrd [already installed]
  - rootfs

[skip] crosvm — not available for darwin-arm64
[skip] vmlinux — already installed
[skip] initrd — already installed
[install] rootfs ...
  Downloading https://cloud-images.ubuntu.com/releases/jammy/release-20260515/ubuntu-22.04-server-cloudimg-arm64.squashfs...
  Checksum verified (sha256)
  Installed: /Users/sash/Library/Application Support/elastos/capsules/ubuntu-base/rootfs.ext4

Done. 1 installed, 3 skipped.

$ file "$HOME/Library/Application Support/elastos/capsules/ubuntu-base/rootfs.ext4"
…/rootfs.ext4: Squashfs filesystem, little endian, version 4.0, xz compressed, 431012242 bytes, 43314 inodes, blocksize: 131072 bytes, created: Fri May 15 11:00:23 2026
```

Wall-clock: ~55 s on this connection.

## Operator note: stamped-manifest shadowing

The first `elastos setup` run after these changes reported only the
pre-existing `vmlinux` + `initrd` + `crosvm` plan and skipped
`rootfs`. Root cause: `setup.rs:328` (`load_manifest()`) prefers a
stamped `components.json` in the data dir over the repo's copy, and
the stamped copy was written by the Phase-7-Day-2 install run before
`rootfs` existed.

For dev iteration on this Mac we backed up the stale stamped copy
once:

```bash
mv "$HOME/Library/Application Support/elastos/components.json" \
   "$HOME/Library/Application Support/elastos/components.json.pre-day2.bak"
```

After re-running setup, the fetcher restamped a fresh copy that
includes `rootfs` (4 grep hits: 1 entry + 3 profiles). End users
won't hit this — they install from a stamped release where the
manifest already includes the new entry. **No code fix required.**
Documented here so the next contributor recognises the pattern.

## Deferred items (NOT Day-2 scope)

- **Day 3 — `elastos doctor` `rootfs` row**: doctor still reports
  only `vmlinux`, `initrd`, `state_dir`, `rootfs_cache_dir`. The
  ubuntu-base rootfs.ext4 doesn't show up. Day 3 will resolve it
  through the supervisor's capsules_dir + add a row matching the
  pattern from Phase 7 Day 4. Quick win once the components-registry
  → doctor wiring path is decided.
- **Pre-existing verifier drift**: `scripts/lib/components-json-verify.sh`
  fails with `[Class C] external.vmlinux.platforms.darwin-arm64.release_path
  missing` — confirmed via `git stash` to pre-date Day 2 (and to
  pre-date Day 1; introduced when Phase 7 Day 1 picked Canonical's
  prebuilt over the `scripts/build-vmlinux-arm64.sh` output the
  Phase-6-Day-4 verifier still expects). Day 2 does not introduce
  any new verifier failures (the verifier doesn't enforce a
  `rootfs` class). Tracked as follow-up: either update the verifier
  to match the Option-B decision, or add `release_path` back as an
  optional field whose absence is now accepted for darwin-arm64.

## Why this is a one-day task

Phase 8 Day 1's audit established that the substrate is already
fully wired (block device FFI, Vz builder, `VmConfig::rootfs_path`,
capsule discovery in the integration test). All that was missing
was the artifact in the install plan. Today's work is JSON +
operator verification — no Rust code changed.

## Next

Phase 8 Day 3 — `elastos doctor` reports the `rootfs.ext4` row and
the `concurrent_load_with_real_kernel` integration test boots end
to end with the now-installed rootfs.
