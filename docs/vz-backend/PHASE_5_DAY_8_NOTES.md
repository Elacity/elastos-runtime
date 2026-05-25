# Phase 5 Day 8 — DRY hoist + perf-schema v2 + Phase-5 closeout

> **Status:** Complete. Phase 5 is now ✅ done.
> **Anchor:** [`PHASE_5_PLAN.md`](PHASE_5_PLAN.md) § Day 8 (revised),
> [`PHASE_5_RETROSPECTIVE.md`](PHASE_5_RETROSPECTIVE.md),
> [`PHASE_6_ENTRY_CHECKLIST.md`](PHASE_6_ENTRY_CHECKLIST.md).
> **Quality gates:** all green (see § Quality gates below).

## Scope deviation from the original plan

The original Day-8 plan (`PHASE_5_PLAN.md` § Day 8, pre-Day-8)
was *"`just verify` Mac parity + Phase 5 closing summary"*.

**Actual Day-8 scope:** DRY hoist + perf-schema v1 → v2 +
Phase-5 closeout (retrospective + Phase-6 entry checklist).

**Rationale.** The original `just verify` Mac-parity scope
re-implements what `mac-vz.yml` already does (Day 5 + Day 6
ship the canonical CI lanes). Chasing it would have
duplicated CI substrate in a developer-facing recipe runner
with no operator-facing benefit. The DRY hoist + schema v2
bump lock in the Phase-5 substrate's stability before
Phase 6 expands it. `just verify` Mac parity moves to the
Phase 6 backlog
([`PHASE_6_ENTRY_CHECKLIST.md`](PHASE_6_ENTRY_CHECKLIST.md) §
Carry-forward).

## What shipped

### 1. Smoke-precedence DRY hoist

**Problem.** Days 5 + 6 introduced inline `if [[ ... ]]; then echo ... fi`
blocks in each of the three Mac smokes to encode the
dry-run precedence. Three smokes × three precedence-related
blocks (FORCE_FULL, CI auto-detect, DRY_RUN-on-exit) = nine
near-identical inline blocks waiting to drift apart.

**Solution.** New `cross_platform_smoke_log_dry_run_reason`
helper in `scripts/lib/cross-platform.sh` consolidates the
"why we entered dry-run" echo logic into a single function.
Together with the Day-6 `cross_platform_smoke_should_dry_run`
predicate, every smoke now uses one canonical pair of calls:

```bash
if [[ "${ELASTOS_VZ_SMOKE_FORCE_FULL:-0}" == "1" ]]; then
    echo "[smoke] FORCE_FULL=1 — forcing full smoke run (overrides CI auto-detect)"
fi

if cross_platform_smoke_should_dry_run; then
    cross_platform_smoke_log_dry_run_reason "[smoke]"
    echo "[smoke] dry-run mode: parse OK, helper sourced OK; exiting before <work>"
    exit 0
fi
```

**Wire-format preservation.** The Day-5/Day-6 inline echo
lines are reproduced byte-for-byte in the production
CI-auto-detect path. Verified via diff against a captured
Day-7 baseline:

```
$ diff /tmp/p5d8-baseline/*.out /tmp/p5d8-post/*.out
✓ chat-wasm-native-interop-smoke.out: byte-identical
✓ home-frontdoor-smoke.out: byte-identical
✓ local-carrier-setup-smoke.out: byte-identical
  (modulo the random `mktemp -d` tempdir suffix in
   local-carrier-setup — unrelated to the refactor)
```

**Additive UX improvement.** The explicit-`DRY_RUN=1` path
now emits a new operator-visible echo
("`[smoke] ELASTOS_VZ_SMOKE_DRY_RUN=1 explicitly set; entering dry-run lane`")
that didn't exist before. The pre-Day-8 explicit-`=1` path
was silent about *why* it was in dry-run; the new helper
makes it loud and clear. This is purely additive for the
local-dev lane; production CI hits the auto-detect branch
which is byte-identical to pre-Day-8.

**Files changed:**
- `scripts/lib/cross-platform.sh` — added `cross_platform_smoke_log_dry_run_reason`.
- `scripts/local-carrier-setup-smoke.sh` — refactored.
- `scripts/home-frontdoor-smoke.sh` — refactored.
- `scripts/chat-wasm-native-interop-smoke.sh` — refactored.

### 2. Shell-helper assertions: 44 → 47

Three new assertions in `scripts/lib/cross-platform-test.sh`
pin the contract of `cross_platform_smoke_log_dry_run_reason`:

1. **Explicit `DRY_RUN=1`** emits the "explicitly set" echo.
2. **CI auto-detect** emits the Day-5 byte-exact "CI detected"
   echo (this is the production CI path that CI log parsers /
   dashboards lean on — the byte-identity guard).
3. **CI auto-detect side-effect** exports `DRY_RUN=1` so any
   smoke code that hasn't been refactored to the predicate
   (none today, but future-proofing) still sees the env var.

```
$ bash scripts/lib/cross-platform-test.sh | tail -3
  OK   log_dry_run_reason on CI exports DRY_RUN=1 side effect
cross-platform.sh: 47 passed, 0 failed
```

### 3. `tests/common/mod.rs` fixture extraction

**Problem.** Day 4's `vz_supervisor_startup_orphan_cleanup.rs`
and Day 7's `vz_perf_harness.rs` carried near-identical
copies of:
- `seed_cached_synthetic_capsule(data_dir, name, cid)`
  (writes the on-disk cache-metadata shape that
  `ensure_capsule` consults to short-circuit the IPFS
  download path);
- `synthetic_components_manifest(name, cid)`
  (builds the in-memory `ComponentsManifest` that the
  supervisor consumes).

**Solution.** Extracted both into
`elastos/crates/elastos-server/tests/common/mod.rs`. The
helper now takes a `description` parameter (rather than
hard-coding "Phase 5 Day 4 …" or "Phase 5 Day 7 …") so each
caller passes its own context. Both files now declare
`mod common;` at the top and import the shared helpers.

This follows the same DRY discipline Phase 5 applied to the
shell smokes (`scripts/lib/cross-platform.sh`) but for the
Rust integration-test substrate.

**Files changed:**
- `elastos/crates/elastos-server/tests/common/mod.rs` (new).
- `elastos/crates/elastos-server/tests/vz_perf_harness.rs` —
  removed inline copies, added `mod common;`.
- `elastos/crates/elastos-server/tests/vz_supervisor_startup_orphan_cleanup.rs` —
  removed inline copies, added `mod common;`.

### 4. Perf-report schema v1 → v2 (additive `git_sha`)

**Problem.** Day 7's perf JSONL records carried `host` +
`notes` + per-record stats but no commit attribution. A
Phase-6 regression-detector consuming the JSON would have
to re-run git to attribute deltas — fragile and slow.

**Solution.** Schema v2 adds a top-level `git_sha` field
(plus per-record `git_sha` on each JSONL line). Schema-v1
consumers ignoring the field still parse v2 files; purely
additive.

**Contract.** The `measure-{vz,crosvm}-baseline.sh`
wrappers capture the workspace git SHA before invoking
`cargo test`:

```bash
PERF_GIT_SHA="$(git rev-parse --short=12 HEAD)"
if [[ -n "$(git status --porcelain)" ]]; then
    PERF_GIT_SHA="${PERF_GIT_SHA}-dirty"
fi
export ELASTOS_VZ_PERF_GIT_SHA="${PERF_GIT_SHA}"
```

The harness reads it via `current_git_sha()` and defaults to
`"unknown"` when the env var is unset (the bare `cargo test`
lane). `"unknown"` is the documented skip-marker the
Phase-6 regression-detector treats as "ignore for
delta-attribution" — keeps dev-host runs honest with a
sentinel rather than an empty string.

**End-to-end verification:**
```
$ ELASTOS_VZ_PERF_RUNS=2 bash scripts/measure-vz-baseline.sh
[measure-vz-baseline] git_sha=fe40122dffab-dirty
...
=== Vz baseline (vz) ===
  git_sha: fe40122dffab-dirty
  ...
$ python3 -c "import json; b=json.load(open('elastos/target/vz-baseline.json')); print(b['schema_version'], b['git_sha'])"
2 fe40122dffab-dirty
```

**Files changed:**
- `elastos/crates/elastos-server/tests/vz_perf_harness.rs` —
  `SCHEMA_VERSION` 1 → 2; added `PerfReport.git_sha`
  + `current_git_sha()` + `PERF_GIT_SHA_ENV`. Existing
  `perf_report_json_schema_is_stable_for_consumers` test
  extended to cover v2 + the default-branch fallback (no new
  test added — test count preserved).
- `scripts/measure-vz-baseline.sh` — captures + exports
  `ELASTOS_VZ_PERF_GIT_SHA`, threads it into the Python
  aggregator, surfaces it in the summary.
- `scripts/measure-crosvm-baseline.sh` — same.
- `docs/vz-backend/PERFORMANCE_BASELINE.md` — § JSON wire
  format documents schema v2.

### 5. `PHASE_5_RETROSPECTIVE.md`

New top-level Phase-5 closeout doc. Sections:
- What we set out to do.
- What we shipped (high-level table, Day 1–8).
- Final state (test counts, CI lanes, perf substrate, docs).
- Scope deviations (Day 5→6, Day 6→7, Day 7 honest scope,
  Day 8 schema-bump rationale).
- What went well (6 items).
- What didn't go well (4 items).
- Carry-forward findings (8 items, lifted into the entry
  checklist).
- Quality-gate trend across all 8 days.
- Phase 5 in one sentence.
- Phase 6 readiness signal.

### 6. `PHASE_6_ENTRY_CHECKLIST.md`

New pre-flight gates doc for Phase 6. Sections:
- Phase 5 closeout gates (9 items, all bash-runnable checks).
- Phase 6 unblockers (3 items, must be resolved before Day 1):
  - `components.json` darwin-arm64 release metadata.
  - Self-hosted Mac CI runner activated.
  - First end-to-end full-boot smoke green.
- Phase 6 backlog (6 items, *not* gates — the starting
  Phase-6 work queue).
- Phase 6 day-1 readiness signal (4 conditions).

### 7. Documentation cascade

- `docs/vz-backend/PLAN.md` § Phase 5 → marked
  "✅ Phase 5 complete" with the Day-8 status block.
- `docs/vz-backend/PHASE_5_PLAN.md` → status updated to
  "All 8 days complete", Day-8 section rewritten to
  reflect actual scope.
- `docs/MAC.md` § Capability matrix → Day-8 row prepended
  with the ✅ marker + the full Day-8 deliverable summary.

## What we explicitly did *not* do

These were considered and intentionally deferred:

1. **`just verify` Mac parity.** Original Day-8 scope; moved
   to Phase 6 backlog. `mac-vz.yml` already provides the
   canonical CI lanes; a Justfile duplicate would just be
   another surface to maintain. Phase 6 can re-evaluate
   whether a developer-facing recipe runner is worth the
   maintenance burden once real microVM boots run.

2. **Linux side of the perf comparison table.** The
   `PERFORMANCE_BASELINE.md` comparison table still has
   `_TBD_` cells for the Linux side. Filling them requires
   running `measure-crosvm-baseline.sh` on a Linux host
   under the same conditions; that's a Day-1 Phase-6
   activity (and is in the entry checklist's "regenerate"
   verification step).

3. **Real microVM boot perf measurements.** Both Vz and
   crosvm. Synthetic-only is still Phase-5 honest scope; the
   `notes.real_vz_boot_measured: false` flag stays for
   Phase-5's baselines and flips to `true` in Phase-6 once
   the harness adds the real-boot path.

4. **Refactor `measure-{vz,crosvm}-baseline.sh` to use the
   smoke precedence helpers.** Those scripts have their own
   inline precedence blocks (carried forward from Day 7).
   The Day-8 prompt was explicit that the "wider smoke
   refactor" was out of scope; perf scripts aren't smokes.
   Phase 6 can consider unifying them.

5. **Demo capture (30-second screen recording).** Original
   Day-8 optional deliverable. Skipped — no Linux peer
   readily available, and the smoke + retrospective +
   entry-checklist trio already documents Phase 5's
   functional state better than a demo would. Phase 6 Day 1
   (or later) can record a real-Vz-boot demo once the
   `components.json` metadata is restored.

## Carry-forward findings (Phase 6)

These are repeated from `PHASE_5_RETROSPECTIVE.md` § Carry-
forward findings for convenience. They live as actionable
items in `PHASE_6_ENTRY_CHECKLIST.md`:

1. `components.json` darwin-arm64 release metadata —
   biggest blocker.
2. Real-microVM perf measurement (both Vz and crosvm).
3. Bridge code + `TxExecutable` perf metrics.
4. CI dashboard / regression-detector.
5. Self-hosted runner activation.
6. Smoke FORCE_FULL=1 path exercised end-to-end.
7. `PHASE_N_UNBLOCKERS.md` convention.
8. MCP / agent tooling for the perf harness.

## Quality gates

All gates verified green at end of Day 8.

| Gate | Command | Result |
|------|---------|--------|
| `fmt` | `cd elastos && cargo fmt --all -- --check` | ✓ no diffs |
| `clippy` | `cd elastos && cargo clippy --workspace --all-targets -- -D warnings` | ✓ no warnings |
| Test count preserved | `cargo test -p elastos-server -p elastos-vz --tests` | ✓ 598 tests |
| Helper assertions | `bash scripts/lib/cross-platform-test.sh` | ✓ 47 passed |
| Smoke byte-identical | `diff /tmp/p5d8-baseline/*.out /tmp/p5d8-post/*.out` | ✓ identical (modulo tempdir suffix) |
| Schema v2 | `python3 -c "import json; print(json.load(open('elastos/target/vz-baseline.json'))['schema_version'])"` | ✓ `2` |
| `git_sha` present | same as above, `['git_sha']` | ✓ non-`unknown` |
| Linux untouched | `git diff --stat` on `elastos/crates/elastos-crosvm/` | ✓ no changes |

## Files changed

```
docs/MAC.md                                                     |  1 +
docs/vz-backend/PERFORMANCE_BASELINE.md                         | xx ++--
docs/vz-backend/PHASE_5_DAY_8_NOTES.md                          | (new)
docs/vz-backend/PHASE_5_PLAN.md                                 | xx +-
docs/vz-backend/PHASE_5_RETROSPECTIVE.md                        | (new)
docs/vz-backend/PHASE_6_ENTRY_CHECKLIST.md                      | (new)
docs/vz-backend/PLAN.md                                         |  1 +
elastos/crates/elastos-server/tests/common/mod.rs               | (new)
elastos/crates/elastos-server/tests/vz_perf_harness.rs          | xx +-
elastos/crates/elastos-server/tests/vz_supervisor_startup_orphan_cleanup.rs | xx +-
scripts/chat-wasm-native-interop-smoke.sh                       | xx +-
scripts/home-frontdoor-smoke.sh                                 | xx +-
scripts/lib/cross-platform-test.sh                              | xx +
scripts/lib/cross-platform.sh                                   | xx +
scripts/local-carrier-setup-smoke.sh                            | xx +-
scripts/measure-crosvm-baseline.sh                              | xx +-
scripts/measure-vz-baseline.sh                                  | xx +-
```

## Phase 5 in one sentence

> **Phase 5 turned the Phase-4 Mac substrate from "works
> in lab" to "tested, CI-visible, and benchmarked", with
> honest documentation of what's still Phase-6-gated.**

## What's next

Phase 6 Day 1, gated on
[`PHASE_6_ENTRY_CHECKLIST.md`](PHASE_6_ENTRY_CHECKLIST.md).
