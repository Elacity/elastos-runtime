# Phase 7 Day 5 — Supervisor wires state_dir + rootfs_cache_dir from Mac data dir

**Phase**: 7 (CI lane + artifact publication)
**Day**: 5 (finish the Mac data-dir migration `elastos doctor` surfaced)
**Date**: 2026-05-25
**Status**: GREEN — the macOS supervisor now overrides
`VzConfig::new`'s default `state_dir` and `rootfs_cache_dir` (which
point at the elastos-vz Unix-style `~/.local/share/elastos/...` tree)
with `<data_dir>/vz` and `<data_dir>/rootfs-cache` respectively. On
this Mac all four Vz substrate paths reported by `elastos doctor` now
resolve under the single canonical `~/Library/Application
Support/elastos/` tree. 385/385 elastos-server lib tests + elastos-vz
109/109 (95 unit + 3 integration + 11 smoke) green; Linux byte-
identical.
**Predecessor**: [`PHASE_7_DAY_4_NOTES.md`](./PHASE_7_DAY_4_NOTES.md)
**Successor candidate**: Phase 7 Day 6 — quieter `elastos doctor`
output (route through a WARN-and-above tracing subscriber so the
supervisor's startup-orphan-prune INFO line doesn't bleed into
triage output, Day-4 § 5.2) OR begin Mac runner CI activation.

---

## 1. Headline

`elastos doctor` on this Mac before Day 5 (output extracted from
[`PHASE_7_DAY_4_NOTES.md`](./PHASE_7_DAY_4_NOTES.md) § 5):

```
  vmlinux:     /Users/sash/Library/Application Support/elastos/bin/vmlinux  ✅
  initrd:     /Users/sash/Library/Application Support/elastos/bin/initrd    ✅
  state_dir:  /Users/sash/.local/share/elastos/vz                            ❌ wrong dir
  rootfs_cache_dir:  /Users/sash/.local/share/elastos/rootfs-cache           ❌ wrong dir
```

`elastos doctor` on this Mac after Day 5:

```
  vmlinux:     /Users/sash/Library/Application Support/elastos/bin/vmlinux                ✅
  initrd:     /Users/sash/Library/Application Support/elastos/bin/initrd                  ✅
  state_dir:  /Users/sash/Library/Application Support/elastos/vz                          ✅
  rootfs_cache_dir:  /Users/sash/Library/Application Support/elastos/rootfs-cache         ✅
```

The two newly-migrated directory rows currently print `[absent —
will be created on first launch]` because no real capsule launch has
ever used these Mac paths (the legacy `~/.local/share/elastos/vz` is
where Phase-6 Day-7 boot tests landed). That's correct behaviour —
the supervisor's launcher auto-creates the directories on demand
when the first capsule starts.

## 2. The Day-4 surfacing that motivated this

Day-3 closed a `kernel_path` + `initramfs_path` data-dir mismatch
between `elastos-vz` (Unix-style `~/.local/share/elastos`) and the
`elastos-server` installer (macOS `~/Library/Application
Support/elastos`). It did **not** touch the other two `VzConfig`
fields with platform-relevant defaults: `state_dir` and
`rootfs_cache_dir`.

Day-4's `elastos doctor` inspector — designed precisely to surface
"the launcher resolves a different directory than the installer
writes to" classes of bug — ran on this Mac and immediately showed
the split layout (Day-4 § 5). Per scope-discipline rules, Day-4
documented the finding without fixing it; Day-5 is the dedicated
ticket that closes the loop.

This is the inspector loop working as designed: Day-3 ships a fix,
Day-4 ships an inspector, Day-5 the inspector's first run motivates
its first concrete code change.

## 3. What was added

Two narrow edits, both inside the existing Day-3 macOS `#[cfg]` gate
in `supervisor.rs` `new_with_vz_config`:

### 3.1 Extend the macOS substrate-path block (`+13 LOC` doc + code)

The Day-3 block went from:

```rust
#[cfg(target_os = "macos")]
let vz_config = {
    let vz_config = vz_config.with_kernel_path(kernel_path);
    // ... initrd wire-up ...
};
```

to:

```rust
// Phase 7 Day 5 — directory fields complete the Day-3 data-dir
// migration. ... `elastos doctor` surfaced the split layout on its
// first run (see PHASE_7_DAY_4_NOTES.md § 5). Same
// `#[cfg(target_os = "macos")]` gate as Day 3 → Linux byte-identical.
#[cfg(target_os = "macos")]
let vz_config = {
    let vz_config = vz_config
        .with_kernel_path(kernel_path)
        .with_state_dir(data_dir.join("vz"))
        .with_rootfs_cache_dir(data_dir.join("rootfs-cache"));
    // ... initrd wire-up unchanged ...
};
```

Directory names match the crosvm convention used elsewhere in the
same function (`crosvm_config` uses `data_dir.join("rootfs-cache")`
at line 880) and `VzConfig::new()`'s own defaults — so a single
`elastos doctor` walks a consistent layout on both substrates.

### 3.2 New macOS-only unit test (`+38 LOC`)

`mac_supervisor_wires_state_dir_and_rootfs_cache_dir_from_data_dir`
— minimal surface mirroring the three Day-3 mac tests already in
supervisor.rs:

- Builds a `tempfile::tempdir()` data_dir + empty `ComponentsManifest`.
- Constructs `Supervisor::new(data_dir.clone(), manifest)`.
- Asserts `supervisor.vz_config().state_dir == data_dir.join("vz")`.
- Asserts `supervisor.vz_config().rootfs_cache_dir == data_dir.join("rootfs-cache")`.

An empty manifest is the narrowest test surface: the directory wire-
up happens unconditionally inside `new_with_vz_config`, independent
of any registry entries. Failure messages cite Phase 7 Day 5 +
`PHASE_7_DAY_4_NOTES.md § 5` so a future regression's blame output
links back to the motivation.

## 4. Validation

### 4.1 Compile

```
$ cargo check -p elastos-server
   Checking elastos-server v0.2.0
    Finished `dev` profile [...] in 4.14s
```

Zero warnings.

### 4.2 Targeted macOS supervisor tests (all four)

```
$ cargo test -p elastos-server --lib mac_supervisor
running 4 tests
test supervisor::tests::mac_supervisor_wires_kernel_path_from_registry ... ok
test supervisor::tests::mac_supervisor_wires_state_dir_and_rootfs_cache_dir_from_data_dir ... ok
test supervisor::tests::mac_supervisor_picks_up_installed_initrd_as_default ... ok
test supervisor::tests::mac_supervisor_omits_initramfs_when_not_installed ... ok

test result: ok. 4 passed; 0 failed; ...; 383 filtered out
```

Day-5 test joins the three Day-3 tests cleanly; all four pass under
the same `#[cfg(target_os = "macos")]` gate.

### 4.3 Full elastos-server lib suite (regression check)

```
$ cargo test -p elastos-server --lib
test result: ok. 385 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.13s
```

**385/385**. Pre-Day-5 was 384; the +1 is the new mac test. Zero
regressions across the entire lib surface.

### 4.4 elastos-vz substrate (untouched — sanity check)

```
$ cargo test -p elastos-vz
unit tests:           95 passed; 0 failed
integration tests:     3 passed; 0 failed   (concurrent_launch)
smoke tests:          11 passed; 0 failed
```

109 total, zero failures. Day-5 never touched substrate code; this
just pins the contract that the supervisor-side wire-up did not
inadvertently break the underlying `VzConfig` builder.

### 4.5 Manual smoke (this Mac, branch sash/local-test, debug build)

Full `elastos doctor` output captured in § 1 above. All four
substrate paths now under `~/Library/Application Support/elastos/`.
The directory rows correctly report `[absent — will be created on
first launch]` since this is the first build that resolves them at
the Mac data dir.

## 5. Linux byte-identical proof

The entire Day-5 change is inside the existing `#[cfg(target_os =
"macos")]` block (lines 905-925 in supervisor.rs post-edit). The
`#[cfg(not(target_os = "macos"))] let _ = kernel_path;` discard at
the end of the block is unchanged. On Linux:

- `vz_config` enters and leaves the block as the bare
  `VzConfig::new()` (or whatever the test override passed) — no
  `.with_state_dir` / `.with_rootfs_cache_dir` is ever called.
- The crosvm launch path on Linux continues to read
  `crosvm_config.rootfs_cache_dir` (line 880, unchanged) — the `vz_config`
  directory fields are dead code on Linux.

This matches Day-3's contract. No Linux test suite changes; no
behavioural change on Linux.

## 6. Files changed (full inventory)

| file                                                              | delta              | role                                                  |
|-------------------------------------------------------------------|--------------------|-------------------------------------------------------|
| `elastos/crates/elastos-server/src/supervisor.rs`                 | +51 / -1           | Day-5 macOS wire-up (+13 doc/code) + 1 test (+38)     |
| `docs/vz-backend/PHASE_6_PLAN.md`                                 | +1 status banner   | Day 5 close + Day 6 forward-link                      |
| `docs/vz-backend/PHASE_7_DAY_5_NOTES.md`                          | +new (this file)   | day journal                                           |

Net: ~52 LOC across one supervisor edit + plan banner + this notes
file. No supabase / schema / migration / Vz substrate changes.

## 7. What remains in Phase 7 (after Day 5)

In rough priority order, none blocking end-user features:

1. **Day-6 candidate — quieter doctor output**: route doctor through
   a WARN-and-above tracing subscriber so the supervisor's
   `vz: startup orphan-prune complete` INFO line stops appearing
   between the `data_dir:` row and the `vmlinux:` row in triage
   output. ~5 LOC at the top of `doctor_cmd::run`. (Day-4 § 5.2)
2. **CI Mac runner activation**: scaffolding done in Phase 6 Day 8;
   needs hardware procurement only.
3. **Apple Developer ID signing pipeline**: for distributing
   `elastos-server` to operators outside the dev cohort. Out of scope
   while we're branch-only.
4. **Phase 8 carrier RPC validation**: still requires a bootable
   rootfs image staged on Mac. Day-2 staged the kernel + initrd; the
   rootfs is the missing piece (separate manifest entry, separate
   provenance pipeline).
