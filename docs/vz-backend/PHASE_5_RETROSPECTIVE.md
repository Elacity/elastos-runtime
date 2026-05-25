# Phase 5 — Retrospective

> **Phase 5 closeout.** Anchored in the day-by-day notes
> (`PHASE_5_DAY_{1..8}_NOTES.md`), the Phase 5 plan
> (`PHASE_5_PLAN.md`), and the umbrella plan (`PLAN.md`).
> Cross-reference for Phase 6 entry: `PHASE_6_ENTRY_CHECKLIST.md`.

## What we set out to do

Phase 5 was the *demo + harden* phase. After Phase 4 closed
the typed-error + supervisor-correctness gap, Phase 5 was
chartered to:

1. **Port the existing Linux smoke scripts to Mac.** Three
   smokes (`local-carrier-setup`, `home-frontdoor`,
   `chat-wasm-native-interop`) had to run, dry-run-cleanly,
   and surface any Mac-specific bugs early.
2. **Wire orphan-cleanup into supervisor startup.** Phase 3
   Day 4 had the cleanup primitive; Phase 5 Day 4 made it
   fire on every `Supervisor::new`.
3. **Land a Mac CI lane on GitHub Actions.** Both the GitHub-
   hosted dry-run lane and a self-hosted full-boot lane.
4. **Establish performance baselines and methodology.** Even
   if real Vz boots are Phase-6 gated, the Phase-5 substrate
   measurement contract (schema, methodology, scripts) had
   to be in place.
5. **Quality-tighten the Phase-5 deliverables.** DRY-out
   duplicated helpers, ship retrospective + Phase-6 entry
   checklist.

## What we shipped (high level)

| Day | Deliverable | Outcome |
|-----|-------------|---------|
| 1 | `local-carrier-setup-smoke.sh` Mac port + dry-run lane | ✅ Shipped, dry-run green |
| 2 | `home-frontdoor-smoke.sh` Mac port + helper hoist (`cross_platform_assert_native_binary_release_metadata`) | ✅ Shipped |
| 3 | `chat-wasm-native-interop-smoke.sh` Mac port | ✅ Shipped |
| 4 | Orphan cleanup wired into `Supervisor::new` + Mac-only integration test (`vz_supervisor_startup_orphan_cleanup.rs`) | ✅ Shipped |
| 5 | GitHub Actions CI: `mac-vz.yml` workflow (dry-run lane, helper-tests lane, Rust-tests lane) | ✅ Shipped |
| 6 | Self-hosted CI lane (`mac-vz-full-boot` job) + heartbeat probe (`_self-hosted-probe.yml`) + runner spec | ✅ Shipped (gated; activation deferred to operator) |
| 7 | Perf measurement substrate (`vz_perf_harness.rs`, `measure-{vz,crosvm}-baseline.sh`, `PERFORMANCE_BASELINE.md`) | ✅ Shipped |
| 8 | DRY hoist (precedence helper, `tests/common/mod.rs`, schema v1→v2 with `git_sha`), retrospective, Phase-6 entry checklist | ✅ Shipped |

## Final state

- **Tests:** 598 (macOS dev host) — preserved from Day 7,
  no Linux-side test count changes. Bash helper assertions:
  44 → 47 (Day 8 added 3 for `log_dry_run_reason`).
- **CI lanes (GitHub Actions):** 4 jobs in `mac-vz.yml`
  (parsing-checks × 3 smokes, helper-tests, Rust-tests,
  full-boot — last gated). 1 heartbeat workflow.
- **Perf substrate:** 6 synthetic metrics, JSON schema v2,
  commit-attributed via `git_sha`. No real Vz boot
  measurements yet (Phase-6 gated).
- **Documentation:** 8 day-notes, 1 retrospective, 1
  Phase-6 entry checklist, 1 perf baseline doc, 1 CI
  runbook update, 1 self-hosted runner spec.

## Scope deviations from the original plan

The plan in `PHASE_5_PLAN.md` was day-by-day; here's where
we deviated and why.

### Day 5 → Day 6 split (CI lanes)

**Plan:** "Day 5: Add GitHub Actions macOS CI lane (dry
run only) + helper unit tests + Rust tests."

**Actual:** Day 5 landed the dry-run lane on
GitHub-hosted macOS runners. The self-hosted full-boot
lane shipped on Day 6 (with the heartbeat probe + runner
spec) — pulling the perf-baseline scope from Day 6 into
Day 7 to make room.

**Reason:** Day 5 surfaced that the self-hosted lane
needed its own infrastructure work (runner labels, repo
variables, heartbeat probe). Splitting it kept Day 5's PR
focused on the green-on-day-one dry-run lane.

### Day 6 → Day 7 shift (perf baseline)

**Plan:** "Day 6: Establish performance baseline doc and
methodology."

**Actual:** Day 7 — because Day 6 absorbed the self-hosted
CI work that was originally Day-5 scope.

**Reason:** See above. The cascade was deliberate and
documented in each day's notes.

### Day 7 honest scope: synthetic, not real

**Plan:** "Day 7: Performance baseline establishment."

**Actual:** Day 7 shipped synthetic Rust-level metrics
ONLY. Real Vz boot timings are Phase-6 gated (waiting on
`components.json` darwin-arm64 release metadata).

**Reason:** Honesty over fiction. The harness measures
what's measurable today; `PERFORMANCE_BASELINE.md` ¶ "What
we cannot measure yet" enumerates the unblockers and the
JSON `notes.real_vz_boot_measured: false` flag surfaces
the limitation in every emitted record.

### Day 8 absorbed the schema bump

**Plan:** Day 8 was DRY hoist + retrospective + entry
checklist.

**Actual:** Day 8 also bumped the perf-report schema to
v2 (added `git_sha` field). This was *not* in the original
Day-8 plan but was identified during the Day-7 review as
the smallest possible structural improvement that future-
proofs the Phase-6 regression-detector.

**Reason:** A schema v1 → v2 bump done now (1 day of cost)
saves a v1-only-baseline-rerun-against-v2-tooling
forensic step in Phase 6. The change is purely additive —
v1 consumers ignore the new field; v2-aware consumers
get commit attribution for free.

## What went well

1. **Day-by-day discipline + per-day notes.** Every day
   shipped with `PHASE_5_DAY_N_NOTES.md` capturing scope,
   deliverables, scope deviation, carry-forward findings,
   and quality gates. New contributors can recover full
   context from the notes alone.

2. **Quality gates held every day.** `fmt`, `clippy`, full
   test suite, helper-test assertion count, Linux-untouched
   check — every day enforced the same gate set. Zero
   regressions in 8 days.

3. **DRY discipline carried forward.** Day 2's
   `cross_platform_assert_native_binary_release_metadata`
   hoist set the pattern; Day 6's
   `cross_platform_smoke_should_dry_run` predicate followed
   it; Day 8 finished the job with
   `cross_platform_smoke_log_dry_run_reason` + the perf
   harness `tests/common/mod.rs` extraction.

4. **Honest blockers documented inline.** The "what we
   cannot measure yet" sections of
   `PERFORMANCE_BASELINE.md` and the
   `notes.real_vz_boot_measured: false` field in every
   perf record mean a future maintainer reading the
   baseline JSON or doc never mistakes synthetic numbers
   for real-microVM numbers.

5. **Linux side untouched.** Every Phase 5 change either
   ran on macOS only (e.g. integration tests behind
   `#[cfg(target_os = "macos")]`), was a shared script /
   crate change with byte-identical Linux behaviour, or
   was a doc-only change. `git diff` against the Phase-4
   tag shows the crosvm code path is unchanged at the
   Rust level.

6. **Byte-identical CI output preservation.** The Day-8
   smoke refactor preserved operator-visible echo lines
   byte-for-byte in the production CI path
   (`CI=true` → auto-detect dry-run). Existing CI log
   parsers + dashboards keep working unchanged.

## What didn't go well

1. **Original Day-5/Day-6 plan was too optimistic on CI
   complexity.** Self-hosted lane + heartbeat probe +
   runner spec + repo-variable gating is 1 full day of
   work, not "a few hours of YAML". Resulted in the
   Day-5/6/7 cascade.

2. **Phase-6 unblockers weren't surfaced loudly enough on
   Days 1-3.** The `components.json` darwin-arm64 release
   metadata gap was known throughout, but the smokes
   sometimes paper over it in the dry-run path. Future
   phases should add a `PHASE_N_UNBLOCKERS.md` per phase
   so the gap is always one click away.

3. **Day-7 synthetic harness could be more representative.**
   The 6 metrics cover the dispatch-graph + supervisor
   bootstrap paths but don't cover bridge code or the
   `TxExecutable` flows. Phase 6 should add those metrics
   as the real-microVM-boot work lands.

4. **Self-hosted runner is provisioned but not active.**
   The `mac-vz-full-boot` job is doubly-gated (repo
   variable + runner labels) and won't execute until an
   operator activates it. This is the right *security*
   posture but means the full-boot lane has zero CI
   minutes today. Phase 6 needs to actually run it.

## Carry-forward findings (Phase 6 backlog)

These are items the day-by-day notes called out but that
weren't in Phase-5 scope. They live now in
`PHASE_6_ENTRY_CHECKLIST.md`.

1. **`components.json` darwin-arm64 release metadata.**
   The single biggest blocker. Without it, no smoke can
   exercise the real install path on a Mac, no real Vz
   boot can be timed, and no install lane on the
   self-hosted runner can run end-to-end.

2. **Real-microVM perf measurement.** Today's harness is
   synthetic. Phase 6 expands the harness to include the
   `Supervisor::ensure_capsule` → real `LaunchMicroVm` path
   under both Vz and crosvm, then aggregates wall-clock
   boot times into `target/{vz,crosvm}-baseline.json`.

3. **Bridge code perf metrics.** Phase 4 Day 3 stressed
   the capability-validate path under 1000 parallel calls;
   the Day-7 harness measures that path at 100. Phase 6
   should add bridge code paths
   (`TxExecutable::execute`, the cross-VM RPC dispatch)
   to the synthetic harness once their substrates settle.

4. **CI dashboard / regression detector.** Today the perf
   baseline JSON sits in `target/`; nothing watches it.
   Phase 6 should add a CI job that diffs against a
   committed baseline + alerts on regressions ≥ 20%.

5. **Self-hosted runner activation.** Provision a real
   Apple Silicon machine, install the runner agent per
   `SELF_HOSTED_RUNNER_SPEC.md`, set the repo variable,
   verify the heartbeat probe goes green, then run the
   first full-boot smoke.

6. **Smoke FORCE_FULL=1 path on self-hosted runner.** The
   precedence is wired; nothing's exercised it end-to-end
   yet (Day-6 was infra-only, Day-7 was perf-only). Phase 6
   Day 1 should run the smokes with `FORCE_FULL=1` on the
   self-hosted runner and surface the first real bugs.

7. **`PHASE_N_UNBLOCKERS.md` convention.** Adopt this for
   Phase 6 so the unblockers are always one navigation
   click from the umbrella `PLAN.md`.

8. **MCP / agent tooling for the perf harness.** Once
   real microVM boots run, expose the perf baseline as an
   MCP server endpoint so the agent fleet can query the
   substrate's current state directly.

## Quality gate trend across Phase 5

| Day | fmt | clippy | tests | helper asserts | smoke dry-run |
|-----|-----|--------|-------|----------------|---------------|
| 1   | ✓   | ✓      | 587   | 35             | green         |
| 2   | ✓   | ✓      | 589   | 38             | green         |
| 3   | ✓   | ✓      | 589   | 41             | green         |
| 4   | ✓   | ✓      | 591   | 41             | green         |
| 5   | ✓   | ✓      | 593   | 41             | green         |
| 6   | ✓   | ✓      | 598   | 44             | green         |
| 7   | ✓   | ✓      | 598   | 44             | green         |
| 8   | ✓   | ✓      | 598   | 47             | green (byte-identical to Day-7) |

> Numbers are the *count at end of day*; some days hold the
> Rust count steady (8 fixture refactor, 7 perf-only).

## Phase 5 in one sentence

> **Phase 5 turned the Phase-4 Mac substrate from "works
> in lab" to "tested, CI-visible, and benchmarked", with
> honest documentation of what's still Phase-6-gated.**

## Phase 6 readiness

See `PHASE_6_ENTRY_CHECKLIST.md` for the entry gates.
Tl;dr: ready, with the 8 carry-forward items above as
the starting backlog.
