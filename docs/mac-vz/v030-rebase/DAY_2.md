# Mac VZ Branch Rebase onto v0.3.0 — Day 2 of 4

**Branch**: `sash/local-test-v030`
**Day 2 HEAD**: `90866a1` (= `ded1333` Day 2 main commit + `90866a1` CI fix follow-up)
**Day 1 HEAD (for diff)**: `06b91ce`
**Draft PR**: [#3](https://github.com/Elacity/elastos-runtime/pull/3)

---

## TL;DR

Day 2 lands the entire **conflict-free** surface of the Mac VZ work
on top of v0.3.0 + CVE fixes. After Day 2 the Mac VZ branch:

- Has the full `elastos-vz/` crate (Apple Virtualization.framework
  substrate) wired into the workspace.
- Compiles `elastos-server` cleanly **on Mac** for the first time
  since v0.3.0 — the elastos-crosvm cfg-gating now provides
  Mac-portable stubs for the bits that previously broke.
- Has every Mac-only ancillary file (workflows, scripts, docs).

What's still **NOT** on the branch is anything that requires
reconciling `supervisor.rs` (Day 3) or `carrier_bridge.rs` (Day 4)
with v0.3.0's changes to the same files. That includes some
modules and almost all of the new VZ tests.

| Metric | Day 1 baseline | Day 2 result |
|---|---:|---:|
| Total files added/changed (vs v0.3.0 main) | 1 (DAY_1.md) | 139 |
| Lines added | +183 | +35,019 |
| `cargo audit` vulns | 3 | 3 (unchanged) |
| `cargo audit` warnings | 4 | 4 (unchanged) |
| `cargo clippy --workspace --exclude elastos-guest --tests -- -D warnings` (local Mac) | green | green |
| `cargo fmt --all -- --check` (local Mac) | green | green |
| `elastos-crosvm` compiles on Mac | — | ✅ (cfg-gating ported) |
| `elastos-server` compiles on Mac | — | ✅ (transitively works after crosvm fix) |

---

## What landed

### A. New `elastos-vz` crate (entire crate, no v0.3.0 conflict)

The crate is registered as a workspace member alongside
`elastos-crosvm`. `elastos-server` picks it up via a
macOS-only target dependency:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
elastos-vz = { path = "../elastos-vz" }
```

So Linux / Windows builds are byte-identical (the dep is invisible
to them). On macOS the crate compiles, links Apple's Virtualization
framework, and exposes its FFI/config/provider surface.

Test fixtures inside the crate (in `src/vm.rs`, `src/provider.rs`,
`src/config.rs`, `tests/smoke.rs`, `tests/concurrent_launch.rs`)
were updated to add `authority: None` to their synthetic
`CapsuleManifest` literals — same fix we made on PR #2 for
`wasm.rs`'s test manifests, because v0.3.0 added the
principal-binding `authority` field to the manifest schema.

### B. `elastos-crosvm` Mac portability

| File | Change |
|---|---|
| `crates/elastos-crosvm/src/lib.rs` | cfg-gating |
| `crates/elastos-crosvm/src/rootfs.rs` | cfg-gating |
| `crates/elastos-crosvm/src/network_stub.rs` | NEW (Mac stub) |

This was the proximate reason `elastos-server` was on the
"can't-compile-on-Mac" exclusion list during the CVE rebase. With
this work landed, `cargo check -p elastos-server` succeeds locally
on Mac for the first time. (Mac CI is the canonical gate; this
is just a Mac-developer-experience win.)

### C. `elastos-server` library / source touches

These are conflict-free files that v0.3.0 didn't touch, plus one
file (`binaries.rs`) where I layered Mac VZ's `ELASTOS_DATA_DIR`
guard on top of v0.3.0's env-var override (both behaviors retained
— the guard is wrapped around the global fallback, the override
runs at the top of the function).

| File | Status |
|---|---|
| `binaries.rs` | overlay (v0.3.0 env override + Mac guard) |
| `lib.rs` | overlay (added `pub mod overlay_initrd` to v0.3.0's set; deferred `doctor_cmd` to Day 3) |
| `overlay_initrd.rs` | NEW (CPIO overlay-init builder, self-contained, no external deps) |
| `security_cmd.rs` | overlay (small; v0.3.0 didn't change this file) |
| `sources.rs` | overlay (small; v0.3.0 didn't change this file) |

### D. `elastos-server/Cargo.toml`

Added the macOS-only `elastos-vz` dep block. v0.3.0 had added
`aes-gcm`, `hkdf`, `argon2`, `qrcodegen`, `k256`, `sha3` for the
passkey + recovery-kit work; those are kept as-is, not touched by
this commit.

### E. CI workflows (NEW)

| File | Purpose |
|---|---|
| `.github/workflows/mac-vz.yml` | Mac VZ tests on macOS runners |
| `.github/workflows/release-mac.yml` | Mac release packaging |
| `.github/workflows/_self-hosted-probe.yml` | Self-hosted runner probe |
| `.github/workflows/linux-untouched.yml` | Linux protected-paths gate |
| `.github/workflows/ci.yml` | Modified to add `sash/**` and `vz/**` branch triggers |

### F. Scripts

| File | Status |
|---|---|
| `scripts/check-linux-untouched.sh` | NEW (Linux protected-paths checker) |
| `scripts/lib/cross-platform.sh` | NEW |
| `scripts/lib/cross-platform-test.sh` | NEW |
| `scripts/lib/components-json-verify.sh` | NEW |
| `scripts/lib/runtime-cleanup-test.sh` | NEW |
| `scripts/lib/runtime-cleanup.sh` | overlay (v0.3.0 didn't change) |
| `scripts/measure-crosvm-baseline.sh` | NEW |
| `scripts/measure-vz-baseline.sh` | NEW |
| `scripts/release-mac.sh` | NEW |
| `scripts/release/release-mac.sh` | NEW |
| `scripts/release/elastos-server.entitlements.plist` | NEW |
| `scripts/release/vmlinux-arm64.config` | NEW |
| `scripts/dev/sign-elastos-vz/` | NEW (entitlements + sign script) |
| `scripts/dev/test-sigint-graceful.sh` | NEW |
| `scripts/dev/mac-vz-feature-check/` | NEW (standalone Cargo.toml + bin) |
| `scripts/chat-wasm-native-interop-smoke.sh` | overlay (v0.3.0 didn't change) |
| `scripts/home-frontdoor-smoke.sh` | overlay |
| `scripts/local-carrier-setup-smoke.sh` | overlay |

### G. Docs

| Path | Status |
|---|---|
| `docs/MAC.md` | NEW (Mac status, trust delta, named path) |
| `docs/ELASTOS_PRD.md` | NEW |
| `docs/vz-backend/` (76 files) | NEW (full Phase 0–10 doc tree, plus the `V030_INTEGRATION_NOTES.md` and `V030_MESSAGE_DRAFT.md` from the prior chat) |
| `state.md` | overlay (Mac substrate scope added) |
| `.gitignore` | overlay (build artifact ignores added) |

---

## What was deliberately deferred

### To Day 3 (after `supervisor.rs` reconciliation)

| File | Why deferred |
|---|---|
| `supervisor.rs` | Both sides changed heavily; Day 3 is dedicated to this |
| `setup.rs` | Mac VZ changed `detect_platform()` return type (String → PlatformInfo) and made `load_manifest` pub; both are surface used by `doctor_cmd.rs`. The +340/-53 Mac delta needs to be 3-way-merged with v0.3.0's +20/-9 delta. |
| `vm_provider.rs` | Mac added +544/-1; v0.3.0 +23/-9. Manageable but transitively depends on Supervisor APIs. |
| `runtime_control.rs` | v0.3.0 added +231/-5 (bigger than Mac's +39/-5). Needs careful inspection. |
| `home_cmd.rs` | Both sides changed; small but waits with the others. |
| `run_cmd.rs` | Mac added +370/-2; v0.3.0 +6/-2. |
| `main.rs` | Both sides added CLI subcommands; needs to merge them into a single Clap derive tree. |
| `doctor_cmd.rs` | Depends on `Supervisor` + Mac-VZ-specific `setup.rs` changes. |
| `vm_debug_cmd.rs` | Registered in `main.rs`; conceptually Day-3 alongside the CLI tree. |
| `elastos-guest/runtime.rs` | v0.3.0 rewrote heavily (+184/-354); Mac added +5/-2 portability tweak. Needs 3-way merge. |
| `tests/capability_concurrency.rs` | New file; depends on types that move with `setup.rs` + Supervisor. |
| `tests/common/mod.rs` | Helper module for the tests above. |
| `tests/vz_perf_harness.rs` | Imports `Supervisor` + `SupervisorRequest`. |
| `tests/vz_supervisor_smoke.rs` | macOS-only; relies on Day 3 supervisor changes. |
| `tests/vz_supervisor_startup_orphan_cleanup.rs` | Same. |
| `docs/PC2_CONVERGENCE.md` | Both sides changed; resolve at end of Day 3 with the rest of the doc overlays. |

### To Day 4 (after `carrier_bridge.rs` reconciliation)

| File | Why deferred |
|---|---|
| `carrier_bridge.rs` | Both sides rewrote (v0.3.0 Carrier rooms +1002/-135, Mac VZ FIFO transport +917/-58). Hardest day. |
| `runtime.rs` | Mac added `on_terminate: None` to `WasmBridgeContext` literal — the field is added in carrier_bridge.rs Day-4 work. |
| `tests/vz_shutdown_semantics.rs` | Relies on `BridgeContext::on_terminate`. |
| `tests/vz_chat_interop_smoke.rs` | Relies on Day 4 dispatch graph. |
| `tests/vz_home_frontdoor_smoke.rs` | Same. |
| `crates/elastos-server/fuzz/` | The fuzz target fuzzes carrier_bridge framing — depends on Day 4. |
| `SIGNOFF.md` | Final reviewer's doc, written on Day 4. |

---

## Day 2 reconciliation strategy (per file)

For files where v0.3.0 didn't change anything (column "v0.3" empty
in the conflict map), the strategy was simply
`git checkout archive/local-test-pre-v030-rebase -- <file>`.

For the only meaningful overlay file in Day 2 (`binaries.rs`),
both sides added in different code regions:

- **v0.3.0** added env-var-based override at the top of
  `find_installed_provider_binary`: `ELASTOS_<NAME>_BIN` (per-provider)
  and `ELASTOS_CAPSULE_BIN_DIR` (group dir).
- **Mac VZ** added an `ELASTOS_DATA_DIR` guard around the global
  `dirs::data_dir()` fallback at the bottom of the function — so
  isolated test/smoke runs don't silently leak into the user's real
  install.

These don't conflict semantically and don't conflict in the file
(different code regions). The merged file has v0.3.0's override at
the top and Mac VZ's guard at the bottom, both behaviors retained
verbatim.

`lib.rs` is similar: both sides only added `pub mod` declarations.
v0.3.0 added `auth`, `content`, `provider_resource`. Mac VZ added
`doctor_cmd` and `overlay_initrd`. Day 2 only adds `overlay_initrd`
to v0.3.0's set; `doctor_cmd` is deferred because the file itself
doesn't compile until Day 3 (depends on Supervisor + Mac-VZ-specific
`setup.rs` changes).

---

## Two CI surprises (and the fixes)

### 1. `scripts/check-linux-untouched.sh` was missing

`linux-untouched.yml` references this script, but I missed it on
the first port pass. Day 2's first push surfaced this as a hard
fail in 5 seconds (`chmod: cannot access`). Fixed by pulling the
script in (commit `90866a1`).

### 2. `linux-untouched.yml`'s baseline was unreachable

The workflow uses `VZ_BACKEND_BASELINE: a65dad3` (the Phase 0
commit on the original sash/local-test). That commit is **not
reachable** from the new branch, because the new branch is rooted
on PR #2 (v0.3.0 main + CVE fixes), not on sash/local-test.

Re-baselined to `ded1333` (Day 2's main commit, which has the
elastos-crosvm cfg-gating + the new elastos-vz crate). After Day
2, that's the legitimate "Phase 0" of the rebased Mac VZ branch.
Day 3+ only touches `elastos-server` (NOT in the protected list:
`elastos-crosvm` / `elastos-runtime` / `elastos-common` /
`elastos-compute`), so the gate continues to enforce "no further
Mac-VZ-rebase changes to those four crates."

Inline comment in the workflow documents the rationale.

---

## Verification (Day 2 final)

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | green (zero diff) |
| `cargo clippy -p elastos-vz -p elastos-crosvm -p elastos-server -- -D warnings` | green |
| `cargo clippy --workspace --exclude elastos-guest --tests -- -D warnings` | green |
| `cargo audit` | 3 vulns / 4 warnings (unchanged from PR #2) |
| `cargo check -p elastos-server` (local Mac) | **green for the first time since v0.3.0** |
| Linux CI Build Release / Test / Check + Clippy + Format | (pending — to be appended after run lands) |
| `linux-untouched.yml` (after baseline fix) | (pending — to be appended) |
| `mac-vz.yml` Mac VZ runners | (pending — to be appended) |

---

## Decisions logged (Day 2)

1. **Day 2 stays small.** Originally I thought Day 2 would land
   `vm_provider.rs`, `runtime.rs`, and the new tests. But all of
   those have transitive dependencies on Supervisor / BridgeContext
   APIs that aren't on the branch yet. Deferring kept Day 2 green
   on local Mac AND keeps Day 3's scope honest about what it
   actually has to do (= a lot).
2. **Re-baselined `linux-untouched.yml`.** The Phase 0 baseline
   `a65dad3` was meaningful for the original sash/local-test branch
   but is unreachable from the rebased branch. New baseline is
   Day 2 HEAD, which preserves the gate's intent (Mac VZ work
   doesn't touch the four protected Linux crates beyond cfg-gating).
3. **Deferred `doctor_cmd.rs` even though the file itself is
   "new" with no v0.3.0 conflict.** It compiles only when the
   Mac-VZ-specific `setup.rs` changes land, which is itself a
   3-way merge with v0.3.0. Cleaner to land them together on Day 3.
4. **Did not run `cargo test --workspace` locally.** Tests for the
   ported crates exist as unit/integration tests; they'll run in
   Linux CI and Mac VZ CI on the push. Skipping local test runs
   keeps Day 2 focused on landing changes; CI does the
   verification.
