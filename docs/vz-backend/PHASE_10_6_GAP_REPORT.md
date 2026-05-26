# Phase 10.6 — Substrate CI Cleanup: Partial Completion + Gap Report

**Branch**: `sash/local-test`  
**Date opened**: 2026-05-26  
**Date paused**: 2026-05-26  
**Status**: **PAUSED — partial completion. Two real fixes shipped, four issue classes documented for a future Phase 10.7 cleanup session.**

---

## Why this document exists

Phase 10.5 closed the four medium-severity Vz-substrate findings (M1-M4) and we declared the branch "ready for reviewer." During the Step 2 (inherited-CVE remediation) setup on a separate branch, we did the CI sanity-check we should have done at the end of Phase 10.5 and discovered that this branch's **Linux CI has been continuously red for ~29 hours** (since 2026-05-25T11:46Z, commit `30cccce`).

This document is the honest record of:
1. What that CI red-state actually was (entry baseline).
2. What Phase 10.6 closed in this session (two real fixes, committed and pushed).
3. What Phase 10.6 did NOT close (four distinct unresolved issue classes, itemized).
4. Why we are pausing rather than continuing.
5. Restart preconditions for a future Phase 10.7 session.

---

## Entry-state CI baseline

| Field | Value |
|---|---|
| First continuously-red run | commit `30cccce4` ("Phase 6 Day 6 — Substrate validation milestone"), 2026-05-25T11:46:11Z |
| Duration red on entry | ~29 hours (not 4 days as initially overestimated in conversation) |
| Workflows red on entry | `CI` (Linux build/test/clippy/release), `Mac Vz CI (Phase 5+ Apple Silicon)` |
| Workflow green on entry | `Linux-untouched gate (Vz backend)` (because it only checks a narrow subset of files) |
| Phase 10.5 sign-off claim | "Branch ready for reviewer / M1-M4 closed" — TRUE for M1-M4, but the sign-off **failed to verify** that the broader branch CI was green |

The Phase 10.5 sign-off discipline gap (declaring readiness without checking CI on the same branch) is the root process issue this report flags. The substrate code regressions themselves pre-date Phase 10 entirely — they were introduced during Phase 4/5/6 cross-OS work that nobody noticed because the team-level convention was "Mac CI is what matters; Linux is mainline's job."

---

## What Phase 10.6 closed (two commits, both pushed)

### Closure 1: `cargo fmt` drift across 10 files

- **Commit**: `8c0f54d` ("phase10.6 fix1: cargo fmt --all (zero logic change)")
- **Diff stat**: 10 files, +54/-53 lines, pure rustfmt mechanical output
- **Files affected**:
  - `elastos-server/src/{carrier_bridge.rs,doctor_cmd.rs,lib.rs,overlay_initrd.rs,run_cmd.rs,setup.rs,supervisor.rs}`
  - `elastos-vz/src/config.rs`
  - `elastos-vz/src/ffi/console_forwarder.rs`
  - `elastos-vz/tests/concurrent_launch.rs`
- **Root cause**: Phase 10.5's M3 commit (`4c83a23`) and earlier Phase 9/10 commits left rustfmt-drift. The Mac Vz CI runs `cargo fmt --all -- --check` as its first step and exits red before any other step.
- **Why it shipped**: Local dev loop runs `cargo build` / `cargo test`, not `cargo fmt --check`. Drift was invisible at commit time.
- **Verification**: `cargo check -p elastos-server -p elastos-vz` → green; 411/411 unit tests pass.
- **CI impact**: Closes the `cargo fmt --check` step. (The downstream clippy step still fails — see open issue class #4 below.)

### Closure 2: 5 unconditional `elastos_vz::*` references in `supervisor.rs`

- **Commit**: `1f0e06b` ("phase10.6 fix2: cfg-gate elastos_vz refs in supervisor.rs")
- **Diff stat**: 1 file, +44/-8 lines (adds a `vz_stubs` module + cross-OS type aliases)
- **Pattern applied**:
  ```rust
  #[cfg(target_os = "macos")]
  use elastos_vz::{VzConfig, VzErrorReport};
  #[cfg(not(target_os = "macos"))]
  use vz_stubs::{VzConfig, VzErrorReport};

  #[cfg(not(target_os = "macos"))]
  mod vz_stubs {
      // Zero-data stubs; never constructed at runtime.
  }
  ```
- **Sites converted** (8 total — initial estimate was 5; found 3 more during the audit):
  - `vz_last_error_report()` return type
  - `CapsuleVzErrorOutcome::Found` enum variant
  - `EnsureCapsuleResponse::vz_error` struct field
  - `ok_with_vz_error()` constructor signature
  - `Supervisor::vz_config` struct field
  - `new_with_vz_config()` parameter
  - `new()` caller (`VzConfig::new()`)
  - `vz_config()` public accessor return type
- **Root cause**: `elastos-vz` is correctly target-gated in `Cargo.toml` as a Mac-only dependency, but supervisor.rs referenced it at type-system sites without cfg-gating. Each site failed to resolve on Linux with `error[E0433]: failed to resolve: use of unresolved module or unlinked crate elastos_vz`.
- **Why it shipped**: Same as #1 — local Mac dev never exercised the Linux build path.
- **Verification**: Mac side green (cargo check + 411/411 tests pass). Linux side **not** verifiable locally (cross-compile lacks `x86_64-linux-gnu-gcc` toolchain on this dev box).

---

## What Phase 10.6 did NOT close — four open issue classes

After pushing the two closures above, CI revealed **four additional issue classes** the initial audit missed. Each is documented below with file/line/error/fix pattern so a future session can pick up without re-discovering.

### Open issue #1: `doctor_cmd.rs` has the same kind of unconditional Vz refs

- **File**: `elastos/crates/elastos-server/src/doctor_cmd.rs`
- **Failing sites**:
  - Line 105: `path: &vz_config.kernel_path,` — accesses `.kernel_path` field on `&VzConfig`
  - Line 114: `vz_config.initramfs_path.as_deref()` — accesses `.initramfs_path` field
  - Line 172: `&vz_config.state_dir` — accesses `.state_dir` field
  - Line 173: `&vz_config.rootfs_cache_dir` — accesses `.rootfs_cache_dir` field
  - Line 232: `let probe = elastos_vz::VzConfig::new().with_kernel_path(row.path);` — full unresolved-crate ref
- **CI errors**: `error[E0433]: failed to resolve: use of unresolved module or unlinked crate elastos_vz` (line 232); `error[E0609]: no field 'kernel_path' on type '&VzConfig'` etc. (lines 105-173)
- **Why it's harder than supervisor.rs**: The function `print_report` is `pub(crate)`, not cfg-gated. It calls `supervisor.vz_config()` and reads four fields off the result. On Linux, the supervisor returns the stub `VzConfig` which has no fields.
- **Fix pattern (recommended)**: cfg-gate the whole `print_report` function to Mac, with a Linux stub that prints `"  vz substrate: not available on this platform"` and skips the four artifact rows. This matches the original cross-OS intent ("doctor inspects the Vz substrate; on Linux there is no Vz substrate to inspect").
- **Alternative fix pattern**: extend the Linux `vz_stubs::VzConfig` to carry placeholder fields (`kernel_path: PathBuf`, `initramfs_path: Option<PathBuf>`, `state_dir: PathBuf`, `rootfs_cache_dir: PathBuf`) and a `with_kernel_path(PathBuf) -> Self` builder. More mechanical but produces meaningless doctor output on Linux.
- **Estimated effort**: 30 minutes.

### Open issue #2: `supervisor.rs:1019` leaks one more stub-field access

- **File**: `elastos/crates/elastos-server/src/supervisor.rs:1019`
- **Failing line**: `let _ = vz_config.prune_orphans_on_startup;` inside the body of `pub fn vz_config()` or nearby (verify exact context when restarting).
- **CI error**: `error[E0609]: no field 'prune_orphans_on_startup' on type 'VzConfig'`
- **Why my Phase 10.6 fix missed it**: I cfg-aliased the TYPE references but did not audit field-ACCESS sites against the stub's field set.
- **Fix pattern**: cfg-gate the line to Mac-only:
  ```rust
  #[cfg(target_os = "macos")]
  let _ = vz_config.prune_orphans_on_startup;
  ```
- **Alternative fix**: add `pub prune_orphans_on_startup: bool` to the Linux `vz_stubs::VzConfig`. Lower-blast-radius.
- **Estimated effort**: 5 minutes.

### Open issue #3: `elastos-vz` crate fails its own Linux build

- **File**: `elastos/crates/elastos-vz/src/vm.rs:28`
- **Failing line**: `use crate::error::{VzError, VzExitReason};`
- **CI error**: `error: unused import: 'VzError'` (under `-D warnings`, this is a hard error)
- **Why it fails on Linux**: `VzError` is consumed only inside `#[cfg(target_os = "macos")]` blocks in this file. On Linux the import sits unused. CI's `-D warnings` flag turns the warning into a compile error.
- **Why it's pre-existing**: The crate's lib.rs doc comment claims "Linux / other: the crate still compiles" — that was true at one point. Subsequent Phase 2-6 work added more Mac-gated code without re-checking the Linux unused-import surface.
- **Fix pattern**:
  ```rust
  #[cfg(target_os = "macos")]
  use crate::error::{VzError, VzExitReason};
  ```
  (Or `use crate::error::{VzError as _, VzExitReason as _};` plus per-cfg re-imports.)
- **Scope warning**: Audit ALL files in `elastos-vz/src/` for the same pattern, not just `vm.rs`. Likely 3-5 similar sites.
- **Estimated effort**: 30-60 minutes (depending on audit scope and whether the fix pattern scales cleanly).

### Open issue #4: clippy `manual_find` lint in `concurrent_launch.rs`

- **File**: `elastos/crates/elastos-vz/tests/concurrent_launch.rs:464`
- **Failing block**:
  ```rust
  for marker in MARKERS {
      if lines.iter().any(|l| l.contains(marker)) {
          return Some(marker);
      }
  }
  None
  ```
- **CI error**: `error: manual implementation of Iterator::find` (under `-D warnings`)
- **Why it surfaced now**: Almost certainly because my Phase 10.6 fix1 commit re-formatted this file, which triggered clippy to re-evaluate it. The latent lint was always there but had been cached as pre-existing analysis.
- **Fix pattern (clippy's suggestion)**:
  ```rust
  MARKERS.iter().find(|&marker| lines.iter().any(|l| l.contains(marker))).copied()
  ```
- **Alternative**: `#[allow(clippy::manual_find)]` annotation, with a comment explaining why the explicit loop is preferred for readability in a test.
- **Estimated effort**: 10 minutes.

---

## Restart preconditions for Phase 10.7

When a future session picks this up:

1. **Read this report first**, then `PHASE_10_5_SIGNOFF.md` (which has been updated to point here).
2. **Branch**: `sash/local-test`. Latest commit at pause time: `1f0e06b` ("phase10.6 fix2: cfg-gate elastos_vz refs in supervisor.rs").
3. **Order of operations** (suggested):
   - Open issue #2 first (5 min, isolated, low risk).
   - Open issue #1 next (30 min — pick fix pattern A or B before starting).
   - Open issue #3 third (audit the whole `elastos-vz/src/` tree, not just `vm.rs`).
   - Open issue #4 last (it'll be the only remaining red after the above three).
4. **Each fix should be its own commit** with the same diff-discipline as Phase 10.5 (Cargo.lock untouched, no new deps, explicit verifier in commit body).
5. **Sign-off discipline going forward**: NO phase is "closed" until CI on the same branch is green. Add this to `.cursor/rules/`.

---

## Why we are pausing, not continuing

This pause is **principle-aligned**, not avoidance:

| Principle (from `.cursor/rules/`) | Pause status |
|---|---|
| No Scope Creep — stop on encountering requirements beyond acceptance criteria | **Pause is the rule** |
| Task-Driven Development — no code change without approved task | **Pause restores compliance** (Phase 10.6 is a discovered side-quest, not an approved task) |
| Stop if Blocked / Scope Expanded — document, propose new task, complete only original scope | **Pause IS the rule** |
| Quality Over Speed — proper implementation beats quick hacks | **Continuing = whack-a-mole with multi-cycle CI feedback; pausing = bounded follow-up task** |

The approved work in the current sprint is **Step 2 (inherited CVE remediation on `chore/runtime-cve-hygiene`)**. Phase 10.6 is a discovered cleanup task; the disciplined move is to document it precisely (this report) and return to the agreed scope.

---

## What this pause does NOT undo

- ✅ M1-M4 closures from Phase 10.5 — **unchanged and verified**.
- ✅ Both Phase 10.6 fixes (`8c0f54d`, `1f0e06b`) — **kept on branch, valid partial progress**.
- ✅ Phase 10.5 documentation (Carrier-bridge fuzz harness, sign-off packet) — **unchanged**.
- ❌ Merge-readiness of `sash/local-test` to `main` — **NOT claimed; explicitly retracted in `PHASE_10_5_SIGNOFF.md`**.

---

## Cross-references

- `PHASE_10_5_SIGNOFF.md` — updated with banner pointing here.
- `BRANCH_SUMMARY.md` — updated branch-status callout.
- `docs/vz-backend/cve-hygiene/DAY_1_NOTES.md` — Step 2 Day 1 closure on `chore/runtime-cve-hygiene` (the work we are resuming).

---

## One-line summary

**Two real fixes shipped, four bounded issues documented, branch is honest about its CI red-state, CVE work resumes on its own branch.**
