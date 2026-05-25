# Phase 5 — Hardening + Linux smoke parity on Mac

> **Status:** Days 1–4 complete (see `PHASE_5_DAY_{1,2,3,4}_NOTES.md`); Days 5–8 remain. Day-by-day breakdown of the Phase-5 deliverable from [`PLAN.md`](PLAN.md) ("the Mac substrate is as reliable as the Linux substrate for the same workloads"). Each day lands one commit + one `PHASE_5_DAY_N_NOTES.md` outcome log, following the Phase-4 cadence.
>
> **Day 4 outcome:** `Supervisor::new` now auto-prunes Mac orphan overlays + control sockets + carrier-bridge sockets (split telemetry). Opt-out via `VzConfig::prune_orphans_on_startup = false`. New `SupervisorResponse::orphans_pruned` one-shot field surfaces on the first `EnsureCapsule` response after construction. 3 supervisor unit tests + 2 RPC-contract integration tests + 1 JSON wire-format test, all green. Linux launch path remains byte-identical (stub helper, byte-identical-test passes under `#[cfg(not(target_os = "macos"))]`).
>
> **Anchor:** Phase 4 closed all internal observability surfaces ([`PHASE_4_DAY_8_NOTES.md`](PHASE_4_DAY_8_NOTES.md)). Phase 5 takes that substrate and validates it against the real workloads — the three Linux smoke scripts — and ships the operator-facing polish (startup-time orphan cleanup, perf baseline, CI runner).

---

## 0. Bound + budget

**Calendar bound:** 8 working days (≈ 1.5 weeks). Each day is one focused 5–7 hour deliverable + a notes doc; no day is allowed to grow into a "while we're at it" multi-feature rewrite. If a day's scope expands past 8 hours, stop, document, and propose a follow-up day rather than landing partial work.

**Linux-untouched gate** (`scripts/check-linux-untouched.sh bcf5a0a`) MUST stay green on every commit. The smoke scripts themselves are platform-shared and live under `scripts/` (outside the protected-crate list), but any change touching `elastos-common`, `elastos-compute`, `elastos-crosvm`, or `elastos-runtime/src/{capability,carrier,primitives,trust,...}` requires a CI gate exception and a justified explanation.

**No new external dependencies** without explicit pre-approval (same constraint as Phase 4).

---

## 1. Day-by-day deliverables

### Day 1 — Port `local-carrier-setup-smoke.sh` to Mac (5–7 h)

**Problem.** The existing script is OS-aware in name (`Darwin) OS_TOKEN="darwin"`) but has not been tried on Mac end-to-end. Three concrete blockers we already know about:
1. **`mapfile -t` / `readarray`** appears nowhere in *this* script (it uses a `while IFS= read` loop already) — good. But `home-frontdoor-smoke.sh` does use `mapfile`; we'll port that on Day 2. Day 1 just verifies `local-carrier-setup-smoke.sh` is genuinely bash-3.2 clean.
2. **`pgrep -f` BSD vs GNU semantics.** BSD `pgrep` matches against `argv[0]` only by default; GNU matches the whole command line. The smoke depends on full-line match (`pgrep -f "$root"`). Verify on Mac.
3. **No real Vz launch verification.** The smoke ends with `elastos home --status` + `elastos` (interactive). Neither boots a microVM. We add a real Vz launch step at the tail.

**Concrete deliverables:**
1. **`scripts/local-carrier-setup-smoke.sh`** — audit every `pgrep` / `kill -- "-$pid"` / `ELASTOS_DATA_DIR` codepath against bash 3.2 + BSD utils. Land any fixes inline (small).
2. **New step at smoke tail: real Vz microVM launch.** After `elastos home --status` passes, if `$(uname -s) == Darwin` and a microVM-typed capsule is available (`localhost-provider` is already installed by setup), invoke `elastos capsule localhost-provider --once` (or equivalent supervisor RPC) and assert: (a) launch succeeds within 30 s, (b) `capsule_status` returns `running`, (c) `stop_capsule` returns `last_exit_reason: host_initiated_stop`, (d) `capsule_vz_error` returns `status: ok` with no `vz_error` field (no failure). On Linux, the same step runs against crosvm — proving the smoke now exercises the microVM substrate on **both** platforms.
3. **`scripts/lib/mac-smoke-preflight.sh`** (new) — extracted helper that the Mac smokes share: detects whether `Virtualization.framework` is reachable (writes a `vz_supported.txt` log file), checks that an installed kernel + rootfs are present, and emits a clear skip message if either is missing. Bash 3.2 clean; idempotent.
4. **Tests:**
   - Add a Rust unit test in `elastos-server/src/supervisor.rs` covering the new "smoke launch round-trip" against a synthetic capsule (no real Vz needed — proves the RPC sequence the smoke depends on is contract-stable).
   - Add a shell-level dry-run mode: `ELASTOS_VZ_SMOKE_DRY_RUN=1 scripts/local-carrier-setup-smoke.sh` exits 0 after the build step (no `elastos serve`, no Carrier install). Lets CI prove the script *parses* on Mac before paying for the full run.
5. **Docs:** `docs/vz-backend/PHASE_5_DAY_1_NOTES.md` (new) — what the smoke proves on Mac vs Linux, what gaps surfaced, the new tail step + helper, and the dry-run mode. Update `docs/MAC.md` capability matrix.

**Gates:** `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test -p elastos-server` under both `RUST_TEST_THREADS=1` and `=4`, `scripts/check-linux-untouched.sh bcf5a0a` green. Live smoke run on this Mac if it can be executed without manual intervention; otherwise documented dry-run pass.

---

### Day 2 — Port `home-frontdoor-smoke.sh` to Mac (6–8 h)

**Problem.** This smoke is **not** bash-3.2 clean — it uses `mapfile -t`. It also hits the publisher gateway over HTTPS (which Mac handles fine), runs `elastos setup` against a local source runtime, and then exercises the Home frontdoor by spawning multiple capsule processes. The Home frontdoor on Linux exercises:
- `localhost-provider` as a microVM (needs Vz on Mac).
- `did-provider` as a microVM.
- `webspace-provider` as a microVM.
- The capability-bridge dispatch path from the Phase-4 Day-3 audit.

**Concrete deliverables:**
1. **`scripts/home-frontdoor-smoke.sh`** — replace `mapfile -t temp_pids < <(pgrep -f "$HOME_DIR" || true)` with the bash-3.2 idiom from `local-carrier-setup-smoke.sh` (`while IFS= read -r _pid; do pids+=("$_pid"); done < <(...)`). Add the `*-darwin-*` arch tokens to `host_platform()`.
2. **`scripts/lib/cross-platform.sh`** (new) — single shared file with the bash-3.2-clean helpers: `read_pids_into_array`, `kill_process_group`, `free_port_python3`. Both `local-carrier-setup-smoke.sh` (Day 1) and `home-frontdoor-smoke.sh` (Day 2) source this — DRY across smokes.
3. **Vz-specific assertion at smoke tail:** verify that `did-provider`, `localhost-provider`, and `webspace-provider` actually launched as microVMs (Linux: crosvm; Mac: Vz). Query `capsule_status` for each handle; assert `running`. Then stop each via `stop_capsule`; assert `last_exit_reason: host_initiated_stop`. This is the **architecture-parity proof** — same smoke, same assertions, same RPCs, two substrates.
4. **Tests:** integration test in `elastos-server/tests/vz_home_frontdoor_smoke.rs` (Mac-only) — auto-discovering, visibly-skipping if installed capsules absent. Drives the production path through `SupervisorRequest::LaunchCapsule` against the same three providers the shell smoke uses.
5. **Docs:** `docs/vz-backend/PHASE_5_DAY_2_NOTES.md` + capability matrix bump.

**Risk:** the Home frontdoor on Linux relies on `did-provider`'s SQLite store path. macOS uses a different `dirs::data_dir()` (`~/Library/Application Support` vs `$XDG_DATA_HOME`). Day 2 uses `ELASTOS_DATA_DIR` exclusively to force cross-platform parity — same as Day 1.

---

### Day 3 — Port `chat-wasm-native-interop-smoke.sh` to Mac (6–8 h)

**Problem.** This smoke proves bidirectional WASM↔native message delivery on the *installed packaged path* (it `curl`s `install.sh` from the publisher gateway). On Mac:
- `install.sh` writes binaries to `~/.local/bin` — already cross-platform.
- The smoke spawns `elastos chat` (native, microVM) and `elastos capsule chat-wasm --lifecycle interactive` (wasm).
- The chat-wasm capsule talks to `localhost-provider` via the Phase-4 Day-3 cross-VM dispatch path.

This is **the** smoke that exercises the full Phase-4 Day-3 dispatch graph end to end. If anything in the cross-VM RPC pipeline is wrong, this smoke surfaces it.

**Concrete deliverables:**
1. **`scripts/chat-wasm-native-interop-smoke.sh`** — Mac arch tokens + bash-3.2 audit + the cross-platform helper sourced from Day 2.
2. **Tail assertion:** capture the `vz_error` field on every `stop_capsule` call to ensure no Vz-specific failure was silently swallowed. Phase 4 Day 8's `vz_error` field gives us this — Day 3 makes the smoke **alerting** rather than just **proving**.
3. **Tests:** integration test in `elastos-server/tests/vz_chat_interop_smoke.rs` (Mac-only). Synthetic capsule pair (native chat + WASM chat) wired through a real `ProviderRegistry`; verifies bidirectional message round-trip in <5 s.
4. **Docs:** `docs/vz-backend/PHASE_5_DAY_3_NOTES.md` + capability matrix bump.

**Risk:** the smoke pulls `install.sh` from `https://elastos.elacitylabs.com`. If that gateway is down or returns a Mac-incompatible binary, Day 3 cannot complete its packaged-install assertion. Mitigation: add `ELASTOS_CHAT_INTEROP_OFFLINE=1` flag that skips the gateway install step and uses a locally-built binary instead — same as `ELASTOS_BIN_OVERRIDE` does today, but without requiring two scripts.

---

### Day 4 — Wire `prune_stale_mac_artifacts` into supervisor startup (4–6 h)

**Problem.** Phase 4 Day 5 built `Supervisor::prune_stale_mac_artifacts(&self) -> StaleArtifactCounts` as an opt-in helper. Nothing calls it. After Days 1–3 prove the smoke suite works, the next operator-facing polish is **startup-time orphan cleanup** so a Mac runtime that crashed mid-VM-launch comes back clean on `elastos serve`.

**Concrete deliverables:**
1. **`Supervisor::new`** gains a Mac-only branch that invokes `prune_stale_mac_artifacts` and logs the counts via `tracing::info!`. Configurable via a new `VzConfig::prune_orphans_on_startup: bool` (default `true`) so production can opt out for the dual-supervisor edge case Phase 4 Day 5 called out.
2. **Telemetry:** orphan counts surface as a new `SupervisorResponse::orphans_pruned: Option<OrphanCounts>` on `EnsureCapsule` (the first RPC any client issues against a fresh supervisor). Optional + skip-serialise; Linux returns `None` always.
3. **Tests:** existing `prune_stale_mac_artifacts_removes_overlays_and_sockets_but_preserves_unrelated_files` test stays; add one more covering the supervisor-construction path (verify `Supervisor::new` cleans + logs on macOS, no-ops on Linux).
4. **Docs:** `docs/vz-backend/PHASE_5_DAY_4_NOTES.md` + `docs/MAC.md` crash-recovery section update.

**Out of scope for Day 4:** persistent state for orphan history across supervisor restarts (Phase 4 Day 8 deferred this to Phase 5; we keep it deferred — startup-time cleanup is the immediate need, persistent history is a follow-up).

---

### Day 5 — Concurrent multi-VM stress under real workload (6–8 h)

**Problem.** Phase 4 Day 1 proved 3-VM concurrent launch in unit tests against synthetic capsules. Phase 5 Day 5 proves N-VM concurrent launch + RPC under the real smoke suite. This is the workload that surfaces vsock-CID-allocation bugs, GCD-queue starvation, NSFileHandle resource exhaustion (Apple's NSFileHandle has a process-wide soft limit), and any race conditions in the Day-4-Day-8 startup path.

**Concrete deliverables:**
1. **`elastos-server/tests/vz_concurrent_launch_under_real_workload.rs`** (Mac-only) — auto-discovering, visibly-skipping. Launches 8 concurrent installed capsules (mix: 2× localhost-provider, 2× did-provider, 2× webspace-provider, 2× chat). Asserts: (a) all 8 reach `running` within 90 s, (b) all 8 have distinct vsock CIDs, (c) issuing 5 RPCs from each of 4 consumer tasks (20 RPCs total) sees every nonce paired correctly, (d) stop_capsule on all 8 succeeds with `last_exit_reason: host_initiated_stop`, (e) no `vz_error` surfaces on the happy path.
2. **NSFileHandle exhaustion guard.** Apple's per-process NSFileHandle soft limit is 256 by default. With 8 VMs × ~6 fds each (rootfs, console, vsock, carrier socketpair, log, control) = ~48 fds — well under the limit. But the test invokes `getrlimit(RLIMIT_NOFILE)` first and skips with a clear message if the host's limit is below 128.
3. **Documented baseline.** The test's wall-clock time + the supervisor's reported boot-latency-p50 land in the new `docs/vz-backend/PHASE_5_DAY_5_NOTES.md`. Day 6 turns these into the perf baseline doc.
4. **Docs:** `PHASE_5_DAY_5_NOTES.md` + capability matrix bump.

**Risk:** the test is the first one that requires *real* installed capsules on the host. CI requires the Day-1/2/3 smokes to have run first to install them. Mitigation: visibly-skip if `~/.local/share/elastos/capsules/*` is empty.

---

### Day 6 — Performance baseline document (4–6 h)

**Problem.** PLAN.md L308 asks for "boot latency, throughput vs Linux; declare honest deltas in `docs/MAC.md`." Phase 5 Day 6 measures the deltas honestly, no apologetics.

**Concrete deliverables:**
1. **`scripts/measure-vz-baseline.sh`** (new, Mac-only) — runs the Phase-5-Day-5 stress test 5 times, collects per-launch boot latency (handle minted → `capsule_status: running`), per-launch stop latency (`stop_capsule` issued → `last_exit_reason` returned), and per-RPC round-trip latency. Emits JSON to `target/vz-baseline.json`.
2. **Matching Linux script.** `scripts/measure-crosvm-baseline.sh` runs the same workload against crosvm so the deltas are apples-to-apples. Existing crosvm tests give us the baseline numbers; the script automates collection.
3. **`docs/vz-backend/PERFORMANCE_BASELINE.md`** (new) — table comparing p50 / p99 across the 5 runs for Mac (M1 8C/16G) and Linux (whatever runner we have). Document any delta > 2x with a concrete cause (e.g. "Vz cold-start adds ~700 ms vs crosvm due to NSFileHandle init + Apple's lazy framework load").
4. **`docs/MAC.md`** capability matrix gets a new "Performance vs Linux" row.
5. **Docs:** `PHASE_5_DAY_6_NOTES.md` + the perf baseline doc.

**Out of scope for Day 6:** *fixing* any perf delta. We measure honestly; tuning lands in Phase 6 or a follow-up.

---

### Day 7 — Apple-Silicon GitHub Actions CI runner (6–8 h)

**Problem.** Every Day-1-through-6 deliverable is locally green on the developer's Mac but unproven in CI. PLAN.md L307 asks: "each runs end-to-end on Mac with the Vz backend; each is `green` in CI on an Apple-Silicon GitHub Actions runner."

**Concrete deliverables:**
1. **`.github/workflows/ci.yml`** gains three new jobs:
   - `check-mac` (runs `cargo fmt --check`, `cargo clippy`, `cargo check` on `macos-14` — Apple Silicon).
   - `test-mac` (runs `cargo test --workspace` on `macos-14`).
   - `smoke-mac` (runs Days 1/2/3 smokes back-to-back on `macos-14`).
2. **`.github/workflows/linux-untouched.yml`** unchanged.
3. **Cargo cache.** Use `Swatinem/rust-cache@v2` with `workspaces: elastos` (same as Linux).
4. **Smoke timeout budgets.** Each smoke gets a `timeout-minutes` limit so a wedged Vz call surfaces as a CI failure within reasonable bounds (10 min for Day 1, 15 min for Day 2, 15 min for Day 3).
5. **Visible-skip channel.** Smokes that can't acquire a kernel/rootfs in CI (no published darwin-arm64 vmlinux yet — see Phase 6 risk register) print a clear skip log and exit 0 with a `cargo build --release` proof that the code at least compiles. Visible-skip telemetry counted but not failing.
6. **Docs:** `PHASE_5_DAY_7_NOTES.md` + a new CI section in `docs/MAC.md`.

**Risk:** GitHub Actions' macos-14 runners cost more than ubuntu-latest. Mitigation: smoke jobs run only on `main` and `vz/**` branches (same pattern as Linux), not on every PR push. Documented in the workflow file.

---

### Day 8 — `just verify` Mac parity + Phase 5 closing summary (4–6 h)

**Problem.** PLAN.md success criterion #1: *`just verify` green on `aarch64-apple-darwin`.* PLAN.md L383. Currently `just verify` is Linux-shaped.

**Concrete deliverables:**
1. **`Justfile`** (or top-level recipe runner) gets a Mac-aware `verify` target that invokes the right per-platform smoke set + the Apple-specific quality gates (the Phase-4 Day-7 + Day-8 typed-error tests, the Phase-5 Day-5 concurrent stress).
2. **`state.md` Support boundary** section updated to add macOS (`aarch64-apple-darwin`) — this is the Phase-6 deliverable per the existing PLAN.md L322, but Day 8 prepares the diff so Phase 6 is a strict subset of Phase-5-Day-8.
3. **Phase 5 closing summary** in `docs/vz-backend/PLAN.md` — same as Phase 4 Day 8's capstone summary. Marks Phase 5 complete.
4. **`docs/MAC.md`** capability matrix final pass: every row now refers to Phase-5-or-later. Phase-4 rows get pinned with a "✅ Phase 4 complete" prefix.
5. **Optional: short demo capture.** Phase 4's original deliverable was *"short demo capture showing an unmodified ElastOS MicroVM capsule running in real Vz isolation on a Mac, talking via Carrier to a Linux peer."* Day 8 records a 30-second screen capture if a Linux peer is available; if not, documents that the smoke suite *is* the proof.
6. **Docs:** `PHASE_5_DAY_8_NOTES.md` + Phase-5 capstone in PLAN.md.

---

## 2. Risk register

| Risk | Severity | Mitigation |
|---|---|---|
| `pgrep -f` semantic delta between BSD and GNU surfaces only when a smoke has a long-running child | Medium | Day 1 / Day 2 audit + `scripts/lib/cross-platform.sh` shared helper. |
| macOS bash 3.2 lacks `mapfile` / `readarray` | High | Pre-emptive: every smoke replaces `mapfile` with the `while IFS= read` idiom. Verified on Day 1 (local-carrier) and Day 2 (home-frontdoor). |
| Vz cold-start latency > 2x crosvm | Medium | Day 6 documents honestly per principle #12; we don't tune in Phase 5. |
| Apple-Silicon GitHub runner unavailable for the project's tier | Medium | Day 7 falls back to a self-hosted runner spec — operator runbook in the notes, not a code change. |
| One smoke surfaces a deep Vz substrate bug that takes >1 day to fix | High | Stop the day, file a follow-up day, do NOT scope-creep the originally-planned day. Phase 4 set this precedent (every day was 1 deliverable). |
| Mac-specific `dirs::data_dir()` path collisions with smoke test cleanup | Low | Every smoke uses `ELASTOS_DATA_DIR` exclusively — Day 1 audits. |
| Publisher gateway returns a binary the Mac smoke can't run (e.g. amd64 only) | Medium | Day 3 has an `ELASTOS_CHAT_INTEROP_OFFLINE=1` flag to skip the gateway install. |
| NSFileHandle process-wide soft limit exceeded by Day 5's concurrent stress | Low | Day 5 explicitly checks `RLIMIT_NOFILE` and skips with a clear message. |

---

## 3. Out of scope (deferred to Phase 6)

- Code-signing + notarization (Phase 6 L318).
- Restoring `components.json` darwin entries truthfully (Phase 6 L321).
- `darwin-amd64` (Intel Mac) support (Phase 6 L320).
- Persistent `VzErrorReport` history across supervisor restarts (Phase 4 Day 8 deferral; reaffirmed here).
- Bridged-network (`com.apple.vm.networking`) entitlement code-path (Phase 3 Day 7 made it gated; Phase 5 doesn't unblock it).

---

## 4. Success criteria

By end of Phase 5:
1. All three Linux smokes (`local-carrier-setup-smoke`, `home-frontdoor-smoke`, `chat-wasm-native-interop-smoke`) pass on `aarch64-apple-darwin`.
2. CI runs the three smokes on a `macos-14` runner on every `main` / `vz/**` push.
3. `docs/vz-backend/PERFORMANCE_BASELINE.md` exists with honest Mac vs Linux p50/p99 numbers.
4. `Supervisor::new` performs startup-time orphan cleanup on Mac; `EnsureCapsule` reports the cleanup counts.
5. Concurrent 8-VM launch + RPC round-trip works under real workload (Day 5 test).
6. `just verify` produces a green/red outcome on Mac.
7. Linux launch path byte-identical (`scripts/check-linux-untouched.sh bcf5a0a` green on every commit).
