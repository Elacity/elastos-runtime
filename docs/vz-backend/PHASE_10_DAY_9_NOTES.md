# Phase 10 — Day 9 Notes

**Branch:** `sash/local-test`
**Commits:** `e48a691` (Bug 1), `cee44d5` (Bug 2)
**Date:** 2026-05-26
**Status:** Done — both fixes verified end-to-end on this host.

## Scope

Close the two Mac-specific user-facing bugs surfaced during the Phase 9
Day 6 live walkthrough. Both fixes are small, additive, and individually
committable.

| # | Bug | Symptom | Fix lives in |
|---|-----|---------|--------------|
| 1 | SIGINT/SIGTERM do not stop Vz | `pkill -KILL` needed to clean up | `elastos-server::run_cmd::run_microvm_standalone` |
| 2 | Test binary lacks Vz entitlement after rebuild | `cargo test -p elastos-vz` SIGKILLs | `scripts/dev/mac-local-setup.sh` |

---

## Bug 1 — SIGINT/SIGTERM graceful shutdown for `elastos run`

### Symptom

After the Phase 9 walkthrough, sending SIGINT (Ctrl-C) or SIGTERM to a
foregrounded `elastos run ubuntu-base` did not stop the underlying
`com.apple.Virtualization.VirtualMachine` XPC process. The Vz process
was re-parented to launchd (PPID=1) and stayed alive until the operator
followed up with `pkill -KILL`.

### Root causes

Two distinct issues in `run_microvm_standalone`:

1. **SIGTERM was not handled at all.** The `tokio::select!` loop only
   awaited `tokio::signal::ctrl_c()`, which observes SIGINT but not
   SIGTERM. The default `kill <pid>`, container orchestrators, and
   supervisor scripts all send SIGTERM — none of those triggered the
   shutdown branch, so the loop kept polling status until SIGKILL.

2. **`provider.stop()` was awaited unbounded.** If Apple's
   `VZVirtualMachine.stop` hung (rare but possible under XPC
   back-pressure), the host process hung with it and never reached the
   drop path that triggers `VzMachineHandle::Drop`.

### Fix

`elastos/crates/elastos-server/src/run_cmd.rs` (Mac branch of
`run_microvm_standalone`):

- Install a SIGTERM handler alongside the existing SIGINT handler;
  both arms break out of the same loop, name the trigger in the log
  for diagnosis, and funnel through the same `provider.stop()` call.
- Bound `provider.stop()` with `VZ_STOP_TIMEOUT = 10s`. On timeout, log
  the timeout and fall through to the explicit drop so the
  `VzMachineHandle::Drop` teardown still runs.
- Explicit `drop(provider)` before returning so the Vz teardown happens
  while we can still observe + log it, rather than racing the tokio
  runtime shutdown the CLI dispatch triggers on return.

### Regression harness

`scripts/dev/test-sigint-graceful.sh`:

- Boots a real `ubuntu-base` microVM via `elastos run`.
- Diffs the Vz process set before/after launch to identify the test
  VM. Apple re-parents Vz XPC to launchd, so PPID-based detection
  does not work — the set-diff approach is the only reliable seam.
- Sends SIGINT, waits up to `SHUTDOWN_WAIT_SECS = 15s` for clean
  exit, then asserts the new Vz process is gone (with a 2s grace for
  Apple's XPC teardown flush).
- Repeats with SIGTERM.
- Returns 0 only if both signals produced clean shutdown.

Why a shell script rather than a Rust integration test: spawning
Apple's Vz framework from `cargo test` would require the same
end-to-end bootstrap (signed binary + kernel + rootfs + entitlements)
plus a child-process invocation of this same binary. The shell harness
is the honest seam and runs in ~8s.

### Verification

Before — `elastos run` did not respond to SIGINT, required SIGKILL:

```
$ elastos run ubuntu-base &
$ kill -INT $!
$ ps -A | grep com.apple.Virtualization.VirtualMachine | wc -l
1                                              # ← still alive
$ pkill -KILL com.apple.Virtualization
```

After — clean SIGINT and SIGTERM shutdown, both within ~1s on this
host:

```
$ ./scripts/dev/test-sigint-graceful.sh
--- testing INT shutdown ---
  Vz baseline before launch: 0 process(es)
  elastos run PID: 59460
  new Vz XPC visible after 1s (delta=+1). Sending SIGINT...
  PASS: elastos run + Vz XPC both gone after SIGINT (run=1s)
--- testing TERM shutdown ---
  Vz baseline before launch: 0 process(es)
  elastos run PID: 59595
  new Vz XPC visible after 1s (delta=+1). Sending SIGTERM...
  PASS: elastos run + Vz XPC both gone after SIGTERM (run=1s)

OK: SIGINT and SIGTERM both produced clean shutdown.
```

### Honest scope notes

- `interactive_stdio=true` (TTY mode, where `enable_host_raw_mode_pub`
  returns `Some(guard)`) was not regression-tested here because
  spawning a TTY-attached child in a shell harness is non-trivial.
  Manual smoke: `elastos run ubuntu-base` from a terminal → Ctrl-C →
  process exits in ~1s. Defer automated TTY-mode regression to a
  follow-up task if needed.
- The 10s `VZ_STOP_TIMEOUT` is a guard against pathological hangs, not
  a target latency. Normal teardown completes in ~50ms on M1.

---

## Bug 2 — test-binary auto-resign in `mac-local-setup.sh`

### Symptom

Running `cargo test -p elastos-vz --test concurrent_launch --release`
from a fresh checkout (or after any rebuild of the elastos-vz crate)
failed immediately with:

```
process didn't exit successfully: ...concurrent_launch-...
(signal: 9, SIGKILL: kill)
```

The operator had to manually invoke
`scripts/dev/sign-elastos-vz/sign.sh <test_binary>` after every
rebuild to bestow the `com.apple.security.virtualization` entitlement.

### Root cause

Cargo's linker strips the codesign signature on every relink (Apple
explicitly designed it that way — re-linking invalidates the on-disk
signature). The Day-4 auto-resign loop added in Phase 9 (`mac-local-
setup.sh` section 6) only signed the main `target/debug/elastos`
binary. Integration-test binaries under
`elastos/target/{debug,release}/deps/<name>-<hash>` were never
re-signed, so each `cargo test` rebuild reset them to unsigned.

### Fix

`scripts/dev/mac-local-setup.sh` (two additions, no behavioural change
to existing flows):

- Factor the codesign-check + `sign.sh` invocation into a shared
  `resign_binary_if_missing_entitlement` helper so the main-binary
  loop and the new test-binary loop share one path (DRY: same plist,
  same XML substring check, same dev-sign script).
- Add `resign_vz_test_binaries_for_profile`, which walks
  `elastos/target/<profile>/deps/` for each name in the explicit
  allow-list `ELASTOS_VZ_TEST_BINARIES=(concurrent_launch smoke)`.
  Filters out `*.d` dep-info sidecars and directories, then re-signs
  any executable Mach-O that lacks the Vz entitlement.

Why an allow-list rather than `find … deps/*`: `deps/` also holds
compiled dependency rlibs and helper binaries we must not sign. The
list mirrors `elastos/crates/elastos-vz/tests/*.rs` source layout
and is trivially auditable when a new test is added.

### Plist

Unchanged — `scripts/dev/sign-elastos-vz/vz.entitlements.plist`
already covers the four entitlements both the main binary and the
test binaries need:

```
com.apple.security.virtualization
com.apple.security.cs.allow-jit
com.apple.security.cs.allow-unsigned-executable-memory
com.apple.security.cs.disable-executable-page-protection
```

Tests load both Apple's Vz framework and `wasmtime` indirectly through
the crate's build matrix, so the same entitlement set applies. No
parallel plist needed.

### Verification (fresh-build simulation on this host)

Step 1 — strip the signature (simulates what cargo's relink does):

```
$ codesign --remove-signature \
    elastos/target/release/deps/concurrent_launch-3b05dc6f118ab93d
```

Step 2 — confirm the bug reproduces:

```
$ cargo test -p elastos-vz --test concurrent_launch --release
process didn't exit successfully: ...
(signal: 9, SIGKILL: kill)
```

Step 3 — apply the fix:

```
$ ./scripts/dev/mac-local-setup.sh
[mac-local-setup] release/concurrent_launch-3b05dc6f118ab93d
                  missing Vz/JIT entitlements — re-signing
  Done. ...can now drive Apple's Virtualization.framework.
```

Step 4 — re-run the test, no manual `sign.sh`:

```
$ cargo test -p elastos-vz --test concurrent_launch --release
running 3 tests
test concurrent_load_rejections_isolate_per_vm ... ok
test concurrent_load_with_real_kernel ... ok
test single_vm_boots_to_userspace ... ok

test result: ok. 3 passed; 0 failed; 0 ignored
```

### Fresh-checkout flow now works

From a clean `target/`, the documented operator recipe is now a
two-liner with no manual sign.sh anywhere:

```
$ ./scripts/dev/mac-local-setup.sh
$ cargo test -p elastos-vz --test concurrent_launch --release
```

---

## Files touched

| File | Lines changed | Commit |
|------|---------------|--------|
| `elastos/crates/elastos-server/src/run_cmd.rs` | +50 / -8 | `e48a691` |
| `scripts/dev/test-sigint-graceful.sh` | +202 (new) | `e48a691` |
| `scripts/dev/mac-local-setup.sh` | +75 / -4 | `cee44d5` |
| `docs/vz-backend/PHASE_10_DAY_9_NOTES.md` | +THIS_FILE (new) | (this commit) |

## What this does NOT yet cover

- **TTY-mode SIGINT regression** — automated harness covers only the
  non-TTY (headless) lane; TTY mode is manually smoked.
- **SIGHUP / SIGQUIT** — current fix handles SIGINT + SIGTERM only.
  SIGHUP and SIGQUIT still fall through to the default tokio
  behaviour (process exits, Vz orphaned). Out of scope for Day 9 —
  flag if operators report it.
- **Auto-resign for `cargo test -p elastos-server` test binaries** —
  not currently broken because those tests don't drive Vz, but the
  helper now exists if that changes.

---

## Next: Day 11-13

External security review of the new `elastos-vz` LOC and the
Carrier-bridge framing parser. Operator-driven step — agent pauses
here per the Phase 10 plan.
