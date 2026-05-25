# Phase 7 Day 4 — `elastos doctor` resolved-paths inspector

**Phase**: 7 (CI lane + artifact publication)
**Day**: 4 (substrate triage CLI)
**Date**: 2026-05-25
**Status**: GREEN — `elastos doctor` ships a read-only diagnostic that
inspects the supervisor's resolved Vz substrate paths (kernel, initrd,
state_dir, rootfs_cache_dir), runs the elastos-vz guest-kernel sanity
check against the on-disk kernel, and surfaces the exact remediation
command (`elastos setup --profile minimal`) for absent artifacts.
384/384 elastos-server lib tests pass (was 382 pre-Day-4, +2 doctor
tests, zero regressions). Manual smoke on this Mac verified all four
output branches (present-valid, kernel-absent, initrd-absent, verbose).
**Predecessor**: [`PHASE_7_DAY_3_NOTES.md`](./PHASE_7_DAY_3_NOTES.md)
**Successor candidate**: Phase 7 Day 5 — finish the Day-3 data-dir
migration (state_dir / rootfs_cache_dir, see § 5 below — `elastos
doctor` surfaced the deferred bug on first run, exactly as designed).

---

## 1. Headline

`elastos doctor` exists now. On a freshly-set-up Mac:

```
$ elastos doctor
ElastOS doctor — substrate path resolution check
  platform:   darwin-arm64
  data_dir:   /Users/sash/Library/Application Support/elastos

  vmlinux:     /Users/sash/Library/Application Support/elastos/bin/vmlinux
              [present] size 44.9 MB
              [validate] passes guest-kernel sanity check

  initrd:     /Users/sash/Library/Application Support/elastos/bin/initrd
              [present] size 31.5 MB

  state_dir:  /Users/sash/.local/share/elastos/vz
              [present]

  rootfs_cache_dir:  /Users/sash/.local/share/elastos/rootfs-cache
              [present]
```

When `bin/vmlinux` is missing:

```
  vmlinux:     /Users/sash/Library/Application Support/elastos/bin/vmlinux
              [absent]
              → run: elastos setup --profile minimal
```

When `bin/initrd` is missing (supervisor leaves `initramfs_path = None`):

```
  initrd:     not configured
              kernel-only boot path (only valid if vmlinux has built-in virtio drivers)
```

`--verbose` adds the manifest-side metadata (Canonical URL, SHA256,
compression, declared size) to each artifact row — the surface most
operators will reach for when reporting "this checksum doesn't match
what I expected" against a CI-published manifest.

## 2. Why this is the right Day-4 shape

Phase 7 Day 3 closed a latent data-dir mismatch where `VzConfig::new()`
defaulted `kernel_path` to `~/.local/share/elastos/bin/vmlinux` while
the Day-2 installer wrote to `~/Library/Application Support/elastos/`.
That bug was invisible to the user until a `KernelNotFound` came out
of the launcher. `elastos doctor` makes that entire class of bug
**visible without launching anything** — the inspector ground-truths
the supervisor's resolved paths against the filesystem and tells the
operator exactly what to do.

The command is intentionally narrow:

- **Read-only.** No `mkdir`, no `chmod`, no manifest writes. Safe to
  run during a confused triage session against a production data_dir.
- **No network.** No CID lookups, no IPFS reads, no checksum
  reverification of the downloaded bytes. We trust the Day-2 fetcher's
  download-time SHA256 — doctor's job is to report *which* file the
  supervisor would consume next, not to redo provenance.
- **Same construction path as the daemon.** `doctor_cmd::run` calls
  `Supervisor::new(data_dir, manifest)` — the same constructor
  `serve_cmd.rs:355` reaches. Anything that would surface in a real
  launch path will surface here.
- **Cross-platform.** The command runs on Linux too. There it reports
  whatever the supervisor resolves (which on a Linux dev box is the
  same `~/.local/share/elastos/` paths the existing crosvm tests use).
  Both unit tests are platform-neutral by design.

## 3. What was added

### 3.1 New module: `elastos/crates/elastos-server/src/doctor_cmd.rs` (286 LOC)

Module-level summary at the top of the file mirrors the Day-3 inline
documentation style (substrate motivation, why each branch is shaped
the way it is, scope boundary). Key surfaces:

- `pub async fn run(args: DoctorArgs) -> anyhow::Result<()>` — top
  entry point. Resolves the live data_dir + manifest from disk and
  writes to stdout.
- `pub(crate) fn print_report(out: &mut dyn Write, ...)` — the
  testable seam. Tests drive this with a synthetic `data_dir` +
  manifest and capture into a `Vec<u8>` buffer.
- `ArtifactRow` bundle struct — keeps `print_artifact_row`'s
  signature under clippy's `too_many_arguments` threshold and makes
  adding future rows mechanical.
- `human_bytes` — local copy of a size formatter (5 LOC). Kept private
  rather than reaching into `setup.rs`'s formatter because that
  formatter follows a different output-column convention; doctor's
  output should be stable independent of setup's UI churn.

### 3.2 Module wiring

- `elastos/crates/elastos-server/src/lib.rs` — added `pub mod
  doctor_cmd;` (sorted alphabetically next to `documents`,
  `fetcher`, `gateway_cmd`).
- `elastos/crates/elastos-server/src/setup.rs` — bumped
  `fn load_manifest()` to `pub(crate) fn load_manifest()`. No
  callers changed; the visibility widen is the minimum needed for
  `doctor_cmd` to consume the same manifest the dispatcher uses.

### 3.3 CLI surface

`elastos/crates/elastos-server/src/main.rs`:

- New `Commands::Doctor { verbose: bool }` variant, slotted between
  `Commands::Setup` and `Commands::Source` (operator-tooling
  cluster).
- Dispatch case calls
  `elastos_server::doctor_cmd::run(DoctorArgs { verbose }).await?`.

### 3.4 Tests (2 unit tests, both in `doctor_cmd::tests`)

- `doctor_reports_absent_artifact_with_remediation` — empty
  data_dir, fixture manifest, asserts `[absent]` + remediation hint
  + state_dir/rootfs_cache_dir rows still render.
- `doctor_reports_present_artifact_with_size_and_verbose_metadata`
  — stages a non-kernel placeholder at `bin/vmlinux`, asserts
  `[present]` + `[validate FAIL]` (the kernel sanity check
  correctly rejects the placeholder bytes — proving doctor will
  catch a corrupted artifact even when staged at the right path) +
  the verbose URL/compression lines.

Both tests use `tempfile::tempdir()` to isolate from the host data_dir
and are platform-neutral.

## 4. Validation

### 4.1 Compile

```
$ cargo check -p elastos-server
   Checking elastos-server v0.2.0
    Finished `dev` profile [...] in 6.03s
```

Zero warnings.

### 4.2 Unit tests (this Day)

```
$ cargo test -p elastos-server --lib doctor_cmd::
running 2 tests
test doctor_cmd::tests::doctor_reports_absent_artifact_with_remediation ... ok
test doctor_cmd::tests::doctor_reports_present_artifact_with_size_and_verbose_metadata ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 384 filtered out; finished in 0.00s
```

### 4.3 Full elastos-server lib suite (regression check)

```
$ cargo test -p elastos-server --lib
test result: ok. 384 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.20s
```

384/384. Pre-Day-4 was 382; the +2 are the two new doctor tests.
Zero regressions.

### 4.4 Manual smoke (this Mac, branch sash/local-test, debug build)

All four branches exercised end-to-end against the live
`~/Library/Application Support/elastos/bin/` directory the Day-2
fetcher provisioned (vmlinux 44.9 MB + initrd 31.5 MB):

| case                    | how                                                                     | doctor output                                                |
|-------------------------|-------------------------------------------------------------------------|--------------------------------------------------------------|
| present-valid           | as-installed                                                            | `[present] size 44.9 MB` + `[validate] passes`               |
| kernel absent           | `mv bin/vmlinux bin/vmlinux.bak`                                        | `[absent]` + `→ run: elastos setup --profile minimal`        |
| initrd absent           | `mv bin/initrd bin/initrd.bak`                                          | `not configured / kernel-only boot path` (supervisor decided)|
| verbose                 | `elastos doctor --verbose`                                              | adds Canonical URL + SHA256 + `compression: gzip` per row    |

Files restored after each branch. Time on the slowest run (verbose, full
output): **~1.1 s wall**, dominated by clap parsing + the supervisor's
startup orphan-prune (which logs a `vz: startup orphan-prune complete`
INFO line through tracing — see § 5.2).

## 5. What `elastos doctor` immediately surfaced (Day-4 earned its keep)

The very first invocation against this Mac surfaced **a real Day-3-class
deferred bug**, which is precisely what this command was designed to do:

```
  state_dir:  /Users/sash/.local/share/elastos/vz              ← Unix default
  rootfs_cache_dir:  /Users/sash/.local/share/elastos/rootfs-cache  ← Unix default
```

The `vmlinux` + `initrd` paths correctly resolve to
`~/Library/Application Support/elastos/bin/...` (Day-3 fix), but
`state_dir` and `rootfs_cache_dir` are still resolving through
elastos-vz's `default_data_dir()` Unix path. Day-3 wired
`kernel_path` + `initramfs_path` from the registry; it did **not**
override the two directory fields. That's a 2-line `with_state_dir` +
`with_rootfs_cache_dir` call on the same `cfg(target_os = "macos")`
branch as Day-3's kernel wire-up.

**Scope decision today (Day 4):** *do not silently fix this in the
inspector ticket.* The Day-4 scope was the inspector itself; finding
a bug on the first surface is the inspector's whole point and the
fix is a clean Day-5 candidate. Documenting it here, not patching
it, respects the framework rule that "no code shall be changed
unless there is an approved task authorizing that change."

### 5.1 Day-5 proposal (~10 LOC, follows Day-3 pattern exactly)

In `supervisor.rs` `new_with_vz_config` the Day-3 `#[cfg(target_os =
"macos")]` block extends to:

```rust
#[cfg(target_os = "macos")]
let vz_config = {
    let vz_config = vz_config
        .with_kernel_path(kernel_path)
        .with_state_dir(data_dir.join("vz"))
        .with_rootfs_cache_dir(data_dir.join("rootfs-cache"));
    // ... initramfs as before ...
};
```

Plus a doctor-driven test: stage a `tempdir` data_dir, call
`Supervisor::new`, assert `vz_config.state_dir.starts_with(data_dir)`.
Linux is untouched (the cfg-gate stays).

### 5.2 Minor cosmetic: tracing INFO leaks into doctor output

The supervisor logs `vz: startup orphan-prune complete ...` at INFO
during construction. doctor inherits the default tracing subscriber,
so that line appears between the `data_dir:` row and the `vmlinux:`
row. Not wrong — the prune did run — but a triage command should be
quieter by default. Two-line fix (route doctor through a stricter
subscriber than serve, e.g. WARN-and-above). Day-5 candidate; also
not in Day-4 scope.

## 6. Files changed (full inventory)

| file                                                              | delta              | role                                          |
|-------------------------------------------------------------------|--------------------|-----------------------------------------------|
| `elastos/crates/elastos-server/src/doctor_cmd.rs`                 | +286 (new file)    | inspector implementation + 2 unit tests       |
| `elastos/crates/elastos-server/src/lib.rs`                        | +1                 | `pub mod doctor_cmd;`                         |
| `elastos/crates/elastos-server/src/setup.rs`                      | +0 / -0 / vis bump | `fn load_manifest` → `pub(crate) fn`          |
| `elastos/crates/elastos-server/src/main.rs`                       | +18                | clap `Doctor { verbose }` variant + dispatch  |
| `docs/vz-backend/PHASE_6_PLAN.md`                                 | +1 status banner   | Day 4 close + Day 5 forward-link              |
| `docs/vz-backend/PHASE_7_DAY_4_NOTES.md`                          | +new (this file)   | day journal                                   |

Net: ~310 LOC added across one new file + four edits. No supabase /
schema / migration / Vz substrate changes.

## 7. What remains in Phase 7 (after Day 4)

In rough priority order, none blocking end-user features:

1. **Day-5 candidate (immediate)**: finish the data-dir migration
   `state_dir` + `rootfs_cache_dir` (§ 5.1 above, ~10 LOC + 1 test).
   Doctor will then show a fully consistent
   `~/Library/Application Support/elastos/` layout on Mac.
2. **Quieter doctor output**: route doctor through a WARN-level
   tracing subscriber so the supervisor's INFO logs don't bleed into
   triage output (§ 5.2, ~2 LOC).
3. **CI Mac runner activation**: scaffolding done in Phase 6 Day 8;
   needs hardware procurement only.
4. **Apple Developer ID signing pipeline**: for distributing
   `elastos-server` to operators outside the dev cohort. Out of scope
   while we're branch-only.
5. **Phase 8 carrier RPC validation**: still requires a bootable
   rootfs image staged on Mac. Day-2 staged the kernel + initrd; the
   rootfs is the missing piece (separate manifest entry, separate
   provenance pipeline).
