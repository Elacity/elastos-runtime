# Phase 5 — Day 3 — Port `chat-wasm-native-interop-smoke.sh` to Mac + VzError alerting tripwire

> **Status:** Complete. One commit, push deferred.
>
> **Plan reference:** [`PHASE_5_PLAN.md` § Day 3](PHASE_5_PLAN.md#day-3--port-chat-wasm-native-interop-smokesh-to-mac-68-h).
>
> **Anchors:** [`PHASE_5_DAY_1_NOTES.md`](PHASE_5_DAY_1_NOTES.md), [`PHASE_5_DAY_2_NOTES.md`](PHASE_5_DAY_2_NOTES.md), [`PHASE_4_DAY_7_NOTES.md`](PHASE_4_DAY_7_NOTES.md), [`PHASE_4_DAY_8_NOTES.md`](PHASE_4_DAY_8_NOTES.md).

---

## 1. What shipped

### 1.1 `scripts/lib/cross-platform.sh` — two new helpers

| Helper | Purpose | Replaces |
|---|---|---|
| `cross_platform_curl_or_skip <url> [prefix]` | `curl --head --max-time 5` probe. Returns 0 if reachable, 1 with an actionable skip message otherwise. The skip message names the `ELASTOS_CHAT_INTEROP_OFFLINE=1` escape hatch so operators know how to proceed without the gateway. | Inline `curl -fsSL …` blow-ups that gave cryptic errors when the publisher gateway was down. |
| `cross_platform_alert_on_vz_error_in_logs <log…>` | The Day-3 alerting tripwire on the Phase-4-Day-7 `VzError::Display` contract. Greps the supplied log files for any of the nine stable `kind_label:` tokens (`vz_internal:` / `vz_invalid_configuration:` / `vz_invalid_state:` / `vz_invalid_state_transition:` / `vz_network_error:` / `vz_operation_cancelled:` / `vz_not_supported:` / `vz_timed_out:` / `vz_unknown:`). Returns 1 + prints matching lines + runbook pointer if any token found. False-positive-safe: prose mentioning the bare token without the trailing colon does NOT trip the alert. | Smokes that silently passed when a Vz-substrate failure was swallowed by an outer error path. |

**Bash 3.2 audit:** both helpers use `[[ … ]]`, `command -v`, `case`, and `grep -E` — all supported by macOS bash 3.2 and BSD grep. The Phase 5 Day 2 unit-test fixture pattern was reused for the assertions.

### 1.2 `scripts/lib/cross-platform-test.sh` — 11 new assertions

Total assertion count grew **26 → 37** (over the ≥30 prompt-required target). New coverage:

| Block | Assertions | What it locks in |
|---|---:|---|
| `cross_platform_curl_or_skip` | 3 | Reachable URL returns 0 (with soft-skip if host offline). RFC-2606 `.invalid` URL returns 1. Empty URL returns 1. |
| `cross_platform_alert_on_vz_error_in_logs` | 8 | No-args → 1. Missing file → 0 (best-effort). Clean log → 0. Three different `kind_label:` tokens (`vz_timed_out:`, `vz_internal:`, `vz_unknown:`) → 1. Mixed clean+dirty → 1. **False-positive guard:** prose mentioning the bare token without the trailing colon → 0. |

All 37 assertions pass on this Mac (bash 3.2). Runtime: ~800 ms.

### 1.3 `scripts/chat-wasm-native-interop-smoke.sh` — Mac port

| Change | Reason |
|---|---|
| Source `scripts/lib/cross-platform.sh`; define `OS_TOKEN`. | Shared helper library, OS-aware skip paths. |
| Centralise log paths (`INSTALL_LOG`, `SETUP_LOG`, `BUILD_LOG`, `SESSION_LOG`). | The VzError alerting tail needs canonical paths to grep. |
| Add `ELASTOS_VZ_SMOKE_DRY_RUN=1` early-exit. | CI fast lane (matches Day 1 / Day 2). |
| Add publisher-gateway probe via `cross_platform_curl_or_skip` BEFORE any state is written. | Gateway-down detection: gives a clear skip vs a 30 s curl timeout. |
| Add `ELASTOS_CHAT_INTEROP_OFFLINE=1` flag. | Plan-mandated mitigation per `PHASE_5_PLAN.md` Day-3 risk register. Bypasses `curl install.sh`, requires `ELASTOS_BIN_OVERRIDE`. |
| Capture install.sh's exit code; treat non-zero on Mac as a Phase-6 prerequisite skip. | The published install.sh itself is bash-3.2 dirty (real failure observed on this Mac: `GATEWAYS[@]: unbound variable`). Phase 6 deliverable per `PLAN.md` L321. |
| Post-install Mac pre-flight via `cross_platform_assert_native_binary_release_metadata` for `shell`, `localhost-provider`, `did-provider`, `chat`. | Same Phase-6-prerequisite skip pattern as Days 1 + 2. |
| Replace `kill "$pid"` in cleanup with `kill_pid_then_group "$pid" 2`. | Daemonised children that escape the group now die via the SIGKILL escalation. |
| Tee the PTY-control session to `${SESSION_LOG}`; capture `${PIPESTATUS[0]}` for python's exit code. | The VzError alerting tail needs the session output AND we mustn't lose python's exit code through the pipe. |
| Add VzError alerting tail (calls `cross_platform_alert_on_vz_error_in_logs` on every collected log). | Day-3 deliverable: turns the Phase-4-Day-7 `Display` contract into an active smoke-level tripwire. |
| Add Vz substrate readiness probe at the tail (advisory). | Mirrors Day 1 / Day 2. |
| Run the alert hook even on python-harness failure. | A Vz-substrate cause embedded in the logs is the actionable signal; the python failure is just the proximate symptom. |

**Linux byte-identity guarantee:** every new helper invocation is either (a) a no-op on Linux (Mac-specific guard branches), or (b) semantically equivalent to the original inline code (helpers were designed to match the original behaviour). The `kill_pid_then_group` substitution sends the SAME SIGTERM that the original `kill "$pid"` did; the only added behaviour is bounded-grace SIGKILL escalation, which on Linux now waits up to 2 s for clean termination instead of fire-and-forget. The VzError alerting tail runs on Linux too — it just finds nothing because Linux has no Vz substrate to fail.

### 1.4 `elastos-server/tests/vz_chat_interop_smoke.rs` — synthetic chat-interop contract test

**Purpose:** lock in the bidirectional `ProviderRegistry::send_raw` contract that the shell smoke depends on. If this test passes, the dispatch graph the shell smoke depends on works at the API layer. If the shell smoke fails on Mac post-Phase-6 while this test passes, the bug is in the substrate (install.sh, Vz boot path, or Carrier bridge) — NOT in the cross-VM RPC plumbing. This is the Phase-5 contract guard that runs in <50 ms on any host.

**Three tests, all `#[cfg(target_os = "macos")]`:**

1. **`chat_native_and_wasm_round_trip_via_provider_registry`** — Two synthetic providers (`chat-native`, `chat-wasm`) share an in-memory `ChatBus`. Drives native→WASM `send_raw("send"+"recv")` then WASM→native, asserts (a) both round-trips deliver, (b) the bus is drained (no duplicates from copy-instead-of-move bugs), (c) total wall-clock <5 s.
2. **`unknown_scheme_send_raw_returns_no_provider_error`** — Locks in the typed-error surface for missing schemes. The shell smoke would otherwise see a cryptic timeout if the registry silently swallowed unknown schemes.
3. **`provider_send_raw_error_propagates_up_through_registry`** — Phase-4-Day-3 contract: typed `ProviderError` from a registered provider propagates up to the caller verbatim.

**Why no real Vz VMs.** Day 5 (concurrent multi-VM stress) is the test that uses real Vz VMs against real workloads. Day 3's role is the contract-stability tripwire that runs in <50 ms and surfaces regressions before they reach the shell smoke's `curl install.sh | bash` step.

Run time: 0.00 s reported (sub-millisecond per test on M1).

---

## 2. Phase-6 prerequisite — install.sh itself

This is the first day where the Mac pre-flight surfaces a NEW Phase-6 prerequisite: the published `install.sh` at the gateway is not bash-3.2 clean. Specifically observed on this Mac:

```
[interop] install published runtime
bash: line 523: GATEWAYS[@]: unbound variable
```

This is a real Mac-incompat bug in the published install.sh. The Day-3 smoke now surfaces it as an actionable Phase-6-prerequisite skip with the operator-facing message:

```
Mitigation: set ELASTOS_CHAT_INTEROP_OFFLINE=1 + ELASTOS_BIN_OVERRIDE
            to bypass the gateway and use a locally-built binary;
            the WASM↔native interop proof still runs end-to-end
            against the local build.
```

Operator runbook flag for Phase 6: when restoring `darwin-arm64` release metadata to `components.json`, ALSO audit `install.sh` for bash-4-only patterns (`mapfile`, `readarray`, `GATEWAYS[@]` under `set -u`, BSD `pgrep -f` semantics). The Phase-5-Day-1 / -Day-2 helper library (`scripts/lib/cross-platform.sh`) is the reference for bash-3.2-clean patterns.

---

## 3. Test inventory after Day 3

| Test file | Type | Assertions / tests | Mac behaviour |
|---|---|---|---|
| `scripts/lib/cross-platform-test.sh` | Shell unit | **37 assertions** (26 → 37) | All pass on bash 3.2. |
| `scripts/lib/runtime-cleanup-test.sh` | Shell unit | 5 assertions | All pass. |
| `scripts/local-carrier-setup-smoke.sh` | Shell smoke | n/a | Pre-flight skip clean. Dry-run clean. |
| `scripts/home-frontdoor-smoke.sh` | Shell smoke | n/a | Pre-flight skip clean. Dry-run clean. |
| **`scripts/chat-wasm-native-interop-smoke.sh`** | **Shell smoke** | **n/a** | **Install.sh pre-flight skip clean. Dry-run clean. Offline-without-override actionable hint.** |
| `elastos-server/tests/vz_home_frontdoor_smoke.rs` | Rust integration | 1 test | Visibly-skip clean. |
| **`elastos-server/tests/vz_chat_interop_smoke.rs`** | **Rust integration** | **3 tests** | **All pass in <1 ms.** |

---

## 4. Operator runbook — new flags for Day 3

(Day 1 / Day 2 flags unchanged.)

| Flag | Effect | When to use |
|---|---|---|
| `ELASTOS_VZ_SMOKE_DRY_RUN=1` | Skip cargo + curl work; parse + sourced-helper check; exit 0. | CI fast lane on every push. |
| `ELASTOS_VZ_SMOKE_FORCE_PROCEED=1` | Bypass Mac pre-flight checks (components.json metadata + install.sh failures). | Debugging install.sh itself, or post-Phase-6 dry runs. |
| **`ELASTOS_CHAT_INTEROP_OFFLINE=1`** (Day 3) | Skip `curl install.sh`; use `ELASTOS_BIN_OVERRIDE` instead. Requires the override to be set or smoke exits 1 with a clear build hint. | Local development; CI where the publisher gateway is unreachable or known-broken. |
| `ELASTOS_BIN_OVERRIDE=<path>` | Override the binary used by the smoke. Required when `_OFFLINE=1`. | Local development; pre-Phase-6 Mac sessions. |

---

## 5. Carry-forward findings for Day 4

1. **Day 4 target:** wire `Supervisor::prune_stale_mac_artifacts` into `Supervisor::new`. The helper was built in Phase 4 Day 5; Day 4 makes it run on startup so a crashed Mac runtime comes back clean. Per `PHASE_5_PLAN.md` § Day 4.

2. **Install.sh Mac port** is a Phase 6 deliverable, NOT a Phase 5 deliverable. Day 3 surfaces it; Phase 6 fixes it. The carry-forward is the operator-facing message named the bug and pointed at Phase 6 — operators reading the smoke output today know exactly what to do.

3. **`VzError::Display` contract anchored at three layers:** the original Phase-4-Day-7 unit test, the Phase-4-Day-8 wire-format JSON test, and now the Phase-5-Day-3 smoke-level alerting tripwire. Removing the `kind_label:` prefix from `Display` would trip all three layers simultaneously.

4. **The smoke's PIPESTATUS pattern** is the canonical way to capture an exit code through a pipe when the alerting tail needs the log output. Day 4 / Day 5 / Day 6 smokes that grow similar tee-to-log patterns should reuse this idiom — consider extracting into a `tee_and_capture_exit` helper IFF a second smoke needs it.

---

## 6. Quality gates — all green

- `cargo fmt --all -- --check` clean
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo test -p elastos-server` clean under both `RUST_TEST_THREADS=1` and `=4`
- `bash scripts/lib/cross-platform-test.sh` → **37 passed**, 0 failed
- `bash scripts/lib/runtime-cleanup-test.sh` → 5 passed, 0 failed
- `bash scripts/local-carrier-setup-smoke.sh` → Mac pre-flight skip clean
- `ELASTOS_VZ_SMOKE_DRY_RUN=1 bash scripts/local-carrier-setup-smoke.sh` → dry-run clean
- `bash scripts/home-frontdoor-smoke.sh` → Mac pre-flight skip clean
- `ELASTOS_VZ_SMOKE_DRY_RUN=1 bash scripts/home-frontdoor-smoke.sh` → dry-run clean
- `bash scripts/chat-wasm-native-interop-smoke.sh` → install.sh-failure skip clean (Phase 6 prerequisite)
- `ELASTOS_VZ_SMOKE_DRY_RUN=1 bash scripts/chat-wasm-native-interop-smoke.sh` → dry-run clean
- `ELASTOS_PUBLISHER_GATEWAY=https://example.com bash scripts/chat-wasm-native-interop-smoke.sh` → gateway-probe skip clean
- `scripts/check-linux-untouched.sh bcf5a0a` green

---

## 7. Commit + push

- Local commit: ✅ (push deferred — account suspension carryover from Phase 4).
- Phase 5 Day 3 anchor commit hash will be recorded in [`PHASE_5_PLAN.md`](PHASE_5_PLAN.md) once the push gate is cleared.
