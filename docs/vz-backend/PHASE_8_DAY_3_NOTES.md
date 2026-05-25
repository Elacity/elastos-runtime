# Phase 8 Day 3 — `elastos doctor` `rootfs` row + capsule artifact resolution

**Date**: 2026-05-25
**Branch**: `sash/local-test`
**Anchor**: [`PHASE_8_DAY_2_NOTES.md`](./PHASE_8_DAY_2_NOTES.md) (artifact landed) / [`PHASE_7_DAY_4_NOTES.md`](./PHASE_7_DAY_4_NOTES.md) (doctor scaffolding)
**Forward link**: Day 4 will boot the rootfs end to end through Vz via `concurrent_load_with_real_kernel`.

## TL;DR

`elastos doctor` now surfaces a `rootfs:` row between `initrd:` and
`state_dir:`. It reads the install path from the components registry
(`external.rootfs.install_path`, with the standard platform-overrides-
component-default fallback) and reports `[present] size 411.0 MB` for
the Ubuntu squashfs Day-2 staged, or `[absent — run: elastos setup
--profile minimal]` when no capsule has been installed yet.

Zero regressions: **388/388** lib tests pass (+2 from Day-2's 386 —
paired present/absent rootfs tests mirroring the existing
`doctor_reports_absent_artifact_with_remediation` /
`doctor_reports_present_artifact_with_size_and_verbose_metadata`
pattern for vmlinux).

## What shipped (one commit)

### `src/doctor_cmd.rs` — three small, self-contained changes

1. **New `resolve_rootfs_install_path` helper** (12 lines).
   Mirrors the same platform-info-then-component-default fallback the
   install loop uses in `setup.rs`, kept local to doctor so we don't
   need to promote `setup::resolve_install_path` to `pub(crate)` for
   one caller. Returns `None` when the manifest has no `external.rootfs`
   entry, which doctor handles as a distinct "not configured" message
   pointing at `components.json` (not at `elastos setup`).
2. **`rootfs` row rendered through the existing `print_artifact_row`
   helper.** No new render path — the helper's `validate_as_kernel:
   false` branch was already designed to support non-kernel artifacts
   (it's the path the `initrd` row already takes). The row sits
   between `initrd` and `state_dir`, which is the right logical slot:
   substrate artifacts → cache dirs.
3. **Two new paired tests in the existing test module**:
   - `doctor_reports_rootfs_with_remediation_when_absent` — empty
     data_dir, expect `[absent]` + the `elastos setup --profile
     minimal` remediation hint + the canonical
     `capsules/ubuntu-base/rootfs.ext4` install path in the row.
     Includes an explicit "at least two distinct `[absent]` markers"
     assertion so a future refactor that accidentally drops the row
     can't pass by reusing vmlinux's absent marker.
   - `doctor_reports_rootfs_present_with_size` — writes a 2 KiB
     sentinel at the canonical path, asserts the `rootfs:` section
     contains `[present]` + `2.0 KB`, and explicitly asserts
     **absence** of any `[validate]` line. That last assertion is the
     guardrail against a future change that wires the kernel
     validator into the rootfs row by accident.

### `fixture_manifest()` extended

Added an `external.rootfs` entry to the test fixture (same platform
key as the existing `vmlinux` entry; `install_path:
"capsules/ubuntu-base/rootfs.ext4"` at both platform and component
level for fallback coverage). This is shared infrastructure for the
two new tests and any future doctor tests that need a rootfs row to
exist in the manifest.

## Acceptance bar — all met

- [x] `elastos doctor` reports a `rootfs:` row with the resolved
      path, `[present]/[absent]` status, and human-bytes file size.
- [x] When absent, the remediation reads `elastos setup --profile
      minimal` — matches the vmlinux/initrd remediation so the
      operator has a single command to run no matter which row
      tripped.
- [x] When present, the row terminates after the size line — no
      `[validate]` stanza, no false alarms about a "kernel" failing
      its sanity check on a squashfs.
- [x] `--verbose` mode renders the registry's URL + SHA256 +
      `manifest-size` for the rootfs row (proved on this Mac — see
      § Smoke-test log).
- [x] When the manifest has no `external.rootfs` entry at all,
      doctor reports `rootfs:     not configured` with a hint
      pointing at `components.json` (not at `elastos setup`, since
      no install command will help).
- [x] `cargo test -p elastos-server --lib`: **388 passed, 0 failed,
      2 ignored**. +2 over Day-2's 386, no regressions.
- [x] Manual smoke on this Mac: rootfs row reports `[present] size
      411.0 MB`, verbose mode shows the Canonical squashfs URL +
      pinned SHA256.
- [x] One commit.

## Smoke-test log (verbatim, this Mac)

```text
$ ./target/debug/elastos doctor
ElastOS doctor — substrate path resolution check
  platform:   darwin-arm64
  data_dir:   /Users/sash/Library/Application Support/elastos

  vmlinux:     /Users/sash/Library/Application Support/elastos/bin/vmlinux
              [present] size 44.9 MB
              [validate] passes guest-kernel sanity check

  initrd:     /Users/sash/Library/Application Support/elastos/bin/initrd
              [present] size 31.5 MB

  rootfs:     /Users/sash/Library/Application Support/elastos/capsules/ubuntu-base/rootfs.ext4
              [present] size 411.0 MB

  state_dir:  /Users/sash/Library/Application Support/elastos/vz
              [absent — will be created on first launch]

  rootfs_cache_dir:  /Users/sash/Library/Application Support/elastos/rootfs-cache
              [absent — will be created on first launch]
```

`--verbose` adds the manifest metadata block for the rootfs row:

```text
  rootfs:     /Users/sash/Library/Application Support/elastos/capsules/ubuntu-base/rootfs.ext4
              [present] size 411.0 MB
              url:         https://cloud-images.ubuntu.com/releases/jammy/release-20260515/ubuntu-22.04-server-cloudimg-arm64.squashfs
              checksum:    sha256:d2c9bcc0815a02f09293d56c91b0a8f5b16878977bb38a3b2c1b28efd7d1c1fb
              manifest-size: 430985216 bytes
```

## Design notes

### Why "rootfs" lives between "initrd" and "state_dir"

The operator's mental model when triaging a Vz boot failure is:
"does the substrate have a kernel, an initrd, and a root filesystem
to mount?" Those are the three guest-image inputs. Then "are the
runtime directories OK?" — that's the cache dirs. Putting `rootfs`
adjacent to `initrd` keeps the substrate-input block contiguous so
a glance at the report tells the operator whether the guest can be
constructed at all before they think about runtime state.

### Why the path comes from the registry, not `VzConfig`

Kernel and initrd are global to the Vz substrate — every guest uses
the same kernel + initrd, so they live on `VzConfig`. The rootfs is
per-capsule — `VmConfig::from_manifest` resolves a different path
for each capsule. Doctor's "is the system ready to boot a guest"
question is about the **base** rootfs the install plan stages, not
about any particular VM's per-instance overlay. Reading the path
from `manifest.external.rootfs.install_path` (joined to
`data_dir`) is the single, manifest-driven source of truth that
matches what the install loop wrote in the first place. If a future
phase introduces multiple base rootfses (e.g. one per capsule
profile), doctor would grow a small loop over `manifest.external`
entries with a "is this a rootfs" predicate — the row-rendering
helpers are already polymorphic on label, so the surface area is
trivial.

### Why we did NOT promote `setup::resolve_install_path` to `pub(crate)`

Tempting (DRY) and considered. Rejected because:
- The doctor's call site needs platform-name flexibility for tests
  (the report renders with `"test-platform"` while the fixture
  pins entries at `detect_platform()` — fallback to component-level
  is the only path that resolves).
- `setup::resolve_install_path` has been stable since Phase 6 and
  is used by the install loop, the install-state checker, and a
  handful of cache-metadata writers. Promoting it would entangle
  doctor with their evolution.
- Local helper is 12 lines including doc comment. The DRY win was
  smaller than the coupling cost.

If a third caller appears, promotion is a 1-line change. We don't
need it today.

## Deferred items (NOT Day-3 scope)

- **`concurrent_launch.rs` hard-codes `~/.local/share/elastos/`**
  (`elastos-vz/tests/concurrent_launch.rs:197`). On macOS the rootfs
  installs to `~/Library/Application Support/elastos/` (per Phase 7
  Day 3). The integration test will need a small `dirs::data_dir()`
  refactor before it can find the Day-2 install. That's the first
  thing Day-4 has to fix; doctor is already correct.
- **Pre-existing verifier drift** still open from Day 2 (Class-C
  `release_path missing`). Same status — orthogonal cleanup.

## Why this is a one-day task

Day 1 picked the artifact strategy (Canonical squashfs). Day 2 wired
the install plumbing (manifest entry + profiles + smoke). Day 3
extends the operator UX by ~15 lines of doctor code so the artifact
is visible from the triage tool. No new subsystem, no new test
infrastructure — the existing `print_artifact_row` was already
designed to be label-driven and validator-flag-driven precisely so
non-kernel artifacts could slot in.

## Next

Phase 8 Day 4 — fix the integration test's hard-coded data dir and
run `concurrent_load_with_real_kernel` (currently `#[ignore]`d) on
this Mac with the now-installed kernel + initrd + rootfs. That's
the moment the substrate boots a real Linux guest end-to-end with
all three artifacts coming from `elastos setup`.
