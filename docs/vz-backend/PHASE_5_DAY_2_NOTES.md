# Phase 5 — Day 2 — Port `home-frontdoor-smoke.sh` to Mac

> **Status:** Complete. One commit, push deferred.
>
> **Plan reference:** [`PHASE_5_PLAN.md` § Day 2](PHASE_5_PLAN.md#day-2--port-home-frontdoor-smokesh-to-mac-68-h).
>
> **Day-1 anchor:** [`PHASE_5_DAY_1_NOTES.md`](PHASE_5_DAY_1_NOTES.md).

---

## 1. What shipped

### 1.1 `scripts/lib/cross-platform.sh` — three new helpers

| Helper | Purpose | Replaces |
|---|---|---|
| `kill_pid_then_group <pid> [grace_secs=2]` | POSIX-clean SIGTERM-then-SIGKILL with `pid_is_running`-based escalation. Bounded grace, idempotent on empty / non-numeric / dead PIDs. | Ad-hoc `kill -- "-${pid}" \|\| kill "${pid}"` patterns in `home-frontdoor-smoke.sh` and `runtime-cleanup.sh`. |
| `free_port_via_python3` | Bind ephemeral port via `socket.bind(("127.0.0.1", 0))`, print to stdout, release before return. | Inline `python3 - <<PY` blocks in `local-carrier-setup-smoke.sh` (L38-45) and `home-frontdoor-smoke.sh::free_port` (L44-53). |
| `cross_platform_assert_native_binary_release_metadata <components.json> <name…>` | Mac pre-flight that asserts `darwin-arm64` (or `*`) release metadata exists for every named native binary. Linux callers see the host-shaped key (e.g. `linux-amd64`) checked instead. Read-only — no manifest mutation. | Inline Python `manifest.get("external", …)` blocks in Day 1's smoke; now shared. |
| `cross_platform_print_phase6_skip_message` | Operator-facing message printed when the pre-flight fails. Names the phase, the file, and the two escape hatches (`ELASTOS_VZ_SMOKE_DRY_RUN=1`, `ELASTOS_VZ_SMOKE_FORCE_PROCEED=1`). | Inline `cat >&2 <<MSG` block in Day 1's smoke; now shared. |

**Bash 3.2 audit:** all helpers use `[[ … ]]`, `case …`, `for ((i=0; i<N; i++))`, and `eval "arr=()"` — all supported by macOS bash 3.2. Verified via `bash -n` parse + the live `kill_pid_then_group` round-trip in the unit test suite below.

### 1.2 `scripts/lib/cross-platform-test.sh` — 11 new assertions

The test file's total assertion count grew from **15** (Day 1) to **26**. New assertions cover:

- `kill_pid_then_group` — empty/non-numeric/dead PID no-ops; live PID terminated within grace window via a real `sleep 30` spawn + kill.
- `free_port_via_python3` — returns a port in [1024, 65535].
- `cross_platform_assert_native_binary_release_metadata` — six assertions covering missing-Darwin-entry rejection (Mac-host), Darwin-entry success, wildcard `"*"` success on any host, missing-manifest rejection, no-names rejection, plus a Linux-host byte-identical-behaviour assertion that ensures the helper passes for `linux-amd64` entries.

All 26 assertions pass on this Mac (bash 3.2).

### 1.3 `scripts/lib/runtime-cleanup.sh` — `/proc/<pid>` removal

Was: `[[ -e "/proc/${pid}" ]]` and `kill -- "-${pid}" || kill "${pid}"` — both Linux-only patterns.

Now: sources `scripts/lib/cross-platform.sh` (idempotent guard for callers who already sourced it) and uses `pid_is_running` + `kill_pid_then_group`. Linux behaviour byte-identical (the helpers were designed to match `/proc/<pid>`'s liveness semantics on Linux). Mac behaviour now real: dead-pid coords files are removed cleanly; live-PID kills actually land.

### 1.4 `scripts/lib/runtime-cleanup-test.sh` — 5 new assertions

| Assertion | What it proves |
|---|---|
| Missing coords file → no-op return 0 | The cleanup helper is safe to call on a freshly-created `HOME_DIR` that never had a runtime. |
| Live PID killed + coords file removed | The happy path: spawn `sleep 60`, write coords, invoke cleanup, observe both the file removal and the PID death within the 2 s grace. |
| Dead PID coords file removed without kill | PID-99999 case — file is removed, no `kill` is issued. |
| Empty PID coords file removed without `set -u` trip | Defensive: `{"pid": ""}` is removed cleanly. |
| Live PID is killed within the grace window | The Day-1 `pid_is_running` correctness check, re-anchored at the cleanup-helper layer. |

Run with `bash scripts/lib/runtime-cleanup-test.sh` → 5 passed, 0 failed.

### 1.5 `scripts/local-carrier-setup-smoke.sh` (Day 1) — refactored to DRY

Day 1's inline Python pre-flight + inline `cat >&2 <<MSG` were both extracted into `cross_platform_assert_native_binary_release_metadata` + `cross_platform_print_phase6_skip_message`. Side benefit: the Mac pre-flight now runs BEFORE the misleading `echo "[local-carrier-setup] building current binary…"` line, so the operator-facing skip output isn't preceded by a wrong "we're building things" header.

### 1.6 `scripts/home-frontdoor-smoke.sh` — Mac port

| Change | Reason |
|---|---|
| Source `scripts/lib/cross-platform.sh` at top. | DRY across smokes. |
| Define `OS_TOKEN` (`linux` / `darwin` / …). | Pre-flight gating + probe gating. |
| Add `ELASTOS_VZ_SMOKE_DRY_RUN=1` early exit. | CI fast lane: parse + helper-source proof without cargo-build cost. |
| Add Mac pre-flight via `cross_platform_assert_native_binary_release_metadata`. | Phase-6 prerequisite check; clean exit 0 with operator-facing skip message. Names asserted: `shell`, `localhost-provider`, `did-provider`, `webspace-provider`. |
| Add `ELASTOS_VZ_SMOKE_FORCE_PROCEED=1` bypass. | Developer escape hatch (matches Day 1). |
| Replace `mapfile -t temp_pids < <(pgrep -f "$HOME_DIR" \|\| true)` in cleanup with two `while IFS= read -r pid` loops. | Bash 3.2 has no `mapfile`. Also avoids bash-3.2's "empty `"${arr[@]}"` is unbound" trip under `set -u`. The two-pass pattern (SIGTERM then SIGKILL) is preserved; the BSD/GNU `pgrep -f` semantic is host-uniform (full-line match) so behaviour is byte-identical on Linux. |
| Replace `mapfile -t SOURCE_BOOTSTRAP < <(discover_source_bootstrap …)` with `read_pids_into_array`. | Bash 3.2 portability. The helper is generic — its name reflects primary use but the implementation reads any newline-delimited values. |
| Replace `kill "${SOURCE_RUNTIME_PID}"` with `kill_pid_then_group "${SOURCE_RUNTIME_PID}" 2`. | Daemonised children that escaped the group now die via the SIGKILL escalation. |
| Replace `kill -0` with `pid_is_running`. | Matches the Day-1 audit; one source of truth for "is this PID alive?" |
| Extend `host_platform()` with `Darwin-arm64` → `darwin-arm64` and `Darwin-x86_64` → `darwin-amd64` cases. | Post-Phase-6 smoke can run end-to-end on Mac without the pre-flight skip; today the pre-flight always exits before this is reached. |
| Replace `free_port` body with `free_port_via_python3`. | DRY. |
| Add Vz substrate probe at tail. | Mirrors Day 1; advisory diagnostic. |

**Linux byte-identity guarantee:** every `mapfile -t` replacement is semantically equivalent on Linux (bash 4+) — the `while IFS= read` idiom + `read_pids_into_array` produce arrays indistinguishable from `mapfile`'s output. The `kill_pid_then_group` helper sends the same `kill -- "-${pid}" 2>/dev/null \|\| true; kill "${pid}" 2>/dev/null \|\| true` sequence on Linux, identical to the original; the only added behaviour is the bounded grace-loop with SIGKILL escalation, which on Linux now waits up to 2 s for clean termination instead of fire-and-forget. Linux smoke runs on `ubuntu-latest` are unaffected.

### 1.7 `elastos/crates/elastos-server/tests/vz_home_frontdoor_smoke.rs` — Rust RPC contract test

**Purpose:** lock in the supervisor's RPC sequence that the shell-level `home-frontdoor-smoke.sh` depends on. If the contract drifts (response shape, status enum spelling, `last_exit_reason` value, `vz_error` presence on the happy path), this test surfaces the regression at the Rust layer before it reaches the shell smoke.

**Sequence per provider (`localhost-provider`, `did-provider`, `webspace-provider`):**

```
SupervisorRequest::LaunchCapsule { name, config: Null }
  → response_json.handle
  → wait_for_running (30 s budget, 250 ms poll)
SupervisorRequest::StopCapsule { handle }
  → assert status == "ok"
  → assert last_exit_reason == "host_initiated_stop"      (Phase 4 Day 7 contract)
  → wait_for_stopped (10 s budget, 250 ms poll)
SupervisorRequest::CapsuleVzError { handle }
  → assert status == "ok"
  → assert vz_error is absent (skip-serialised on happy path)   (Phase 4 Day 8 contract)
```

**Visibly-skip semantics:** three independent skip paths:

1. `elastos_vz::is_supported() == false` → off Apple Silicon macOS.
2. `discover_data_dir()` returns `None` → no `elastos setup` ever run.
3. Any of the three providers missing `capsule.json` + `rootfs.ext4` → Phase 6 prerequisite not met.

Today, on this Mac, the test skips at path (2) — `~/.local/share/elastos` doesn't exist because the Mac pre-flight (Day 1) blocks `elastos setup` from running. The skip message is the test's primary output; CI dashboards should alert on the skip line as the Phase-6-prerequisite-not-met telemetry.

**Post-Phase 6:** once `components.json` ships truthful `darwin-arm64` release metadata and the kernel + rootfs are available, the test exercises the full Launch → Status → Stop → VzError contract against all three providers and is the architecture-parity proof (same RPCs, same assertions, two substrates).

---

## 2. Phase-6 prerequisite — unchanged from Day 1

`components.json` still lacks `darwin-arm64` release metadata for the four native binaries (`shell`, `localhost-provider`, `did-provider`, `webspace-provider`). Both Day-1 and Day-2 smokes skip cleanly with the same operator-facing message — the message text is now shared via `cross_platform_print_phase6_skip_message`, so a single update closes the loop on both smokes.

---

## 3. Test inventory after Day 2

| Test file | Type | Assertions | Mac behaviour |
|---|---|---|---|
| `scripts/lib/cross-platform-test.sh` | Shell unit | 26 | All pass on bash 3.2. |
| `scripts/lib/runtime-cleanup-test.sh` | Shell unit | 5 | All pass. |
| `scripts/local-carrier-setup-smoke.sh` | Shell smoke | n/a | Pre-flight skip clean. Dry-run clean. |
| `scripts/home-frontdoor-smoke.sh` | Shell smoke | n/a | Pre-flight skip clean. Dry-run clean. |
| `elastos-server/tests/vz_home_frontdoor_smoke.rs` | Rust integration | 1 test | Visibly-skip clean on this Mac. |

---

## 4. Operator runbook — new flags for Day 2

(Identical surface to Day 1; the shared helper means one description suffices.)

| Flag | Effect | When to use |
|---|---|---|
| `ELASTOS_VZ_SMOKE_DRY_RUN=1` | Skip all cargo work; verify the script parses + sources its helpers; exit 0. Mac runs the Vz host-capability probe before exiting. | CI fast lane on every push; local sanity check after editing a smoke. |
| `ELASTOS_VZ_SMOKE_FORCE_PROCEED=1` | Bypass the Mac pre-flight check even when `components.json` lacks darwin-arm64 entries. | Debugging the WASM/data half of a smoke without waiting for Phase 6. |
| `ELASTOS_VZ_SMOKE_DATA_DIR=<path>` | Override the Rust test's data-dir auto-discovery. | Running the integration test against a non-standard install location. |

---

## 5. Carry-forward findings for Day 3

1. **`chat-wasm-native-interop-smoke.sh`** is the Day-3 target. Initial scan: it does NOT use `mapfile` directly. It DOES `curl install.sh` from the publisher gateway — the offline `ELASTOS_CHAT_INTEROP_OFFLINE=1` flag is the documented mitigation per `PHASE_5_PLAN.md` § Day 3.

2. **Day-2 risk follow-up.** The plan's Day-2 risk note: "macOS uses a different `dirs::data_dir()` (`~/Library/Application Support` vs `$XDG_DATA_HOME`). Day 2 uses `ELASTOS_DATA_DIR` exclusively to force cross-platform parity — same as Day 1." Confirmed: `home-frontdoor-smoke.sh` already sets `XDG_DATA_HOME` on every `elastos` invocation; no Library-path leakage was observed.

3. **Day-3 needs a sixth helper:** `assert_curl_or_skip <url>` — the chat-wasm smoke depends on the publisher gateway being reachable. Day 3 will hoist this into `cross-platform.sh` as the next iteration of the helper library.

4. **Phase 4 Day 8's `vz_error` field is now actively asserted** by the Day-2 Rust integration test. Any future regression in the skip-serialise contract on the happy path surfaces here.

---

## 6. Quality gates — all green

- `cargo fmt --all` clean
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo test -p elastos-server` clean under both `RUST_TEST_THREADS=1` and `=4`
- `bash scripts/lib/cross-platform-test.sh` → 26 passed
- `bash scripts/lib/runtime-cleanup-test.sh` → 5 passed
- `bash scripts/local-carrier-setup-smoke.sh` → Mac pre-flight skip clean
- `ELASTOS_VZ_SMOKE_DRY_RUN=1 bash scripts/local-carrier-setup-smoke.sh` → dry-run clean
- `bash scripts/home-frontdoor-smoke.sh` → Mac pre-flight skip clean
- `ELASTOS_VZ_SMOKE_DRY_RUN=1 bash scripts/home-frontdoor-smoke.sh` → dry-run clean
- `scripts/check-linux-untouched.sh bcf5a0a` green

---

## 7. Commit + push

- Local commit: ✅ (push deferred — account suspension carryover from Phase 4).
- Phase 5 Day 2 anchor commit hash will be recorded in [`PHASE_5_PLAN.md`](PHASE_5_PLAN.md) once the push gate is cleared.
