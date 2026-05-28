# Phase 6 — Entry Checklist

> **Purpose.** Pre-flight gates the Phase 6 lead must
> verify before starting Phase 6 day-1 work. Each gate is
> a concrete artifact / state check, not a "feel good"
> bullet. Anchored in `PHASE_5_RETROSPECTIVE.md` and the
> umbrella `PLAN.md` § Phase 6.
>
> **When to use.** First action of Phase 6: walk this
> checklist top-to-bottom. Any failed gate blocks Phase 6
> kick-off until resolved.

## Phase 5 closeout gates

These verify Phase 5 actually closed cleanly. None should
fail; if any does, **finish Phase 5 first**.

- [ ] **All Phase 5 day-notes present.**
      Check `ls docs/vz-backend/PHASE_5_DAY_*_NOTES.md`
      returns 8 files (Day 1–8).

- [ ] **`PHASE_5_RETROSPECTIVE.md` present.**
      Sibling file to this one.

- [ ] **`PHASE_5_PLAN.md` shows all 8 days status =
      Done.** Open the plan, walk the day-by-day table,
      every row marked complete.

- [ ] **`PLAN.md` Phase 5 section closed.**
      Section header reads "Phase 5 — done" (not "in
      progress").

- [ ] **Test count green.**
      `cd elastos && cargo test -p elastos-server -p elastos-vz --tests`
      → 598 tests pass on macOS, 0 failures. Linux side
      tests untouched (run `git diff phase4-tag..HEAD --
      'elastos/crates/elastos-crosvm/'` should return
      empty).

- [ ] **Helper assertions green.**
      `bash scripts/lib/cross-platform-test.sh`
      → "47 passed, 0 failed".

- [ ] **All three Mac smokes dry-run green.**
      `CI=true bash scripts/local-carrier-setup-smoke.sh`,
      `CI=true bash scripts/home-frontdoor-smoke.sh`,
      `CI=true bash scripts/chat-wasm-native-interop-smoke.sh`
      → all exit 0, all print
      "dry-run mode: parse OK".

- [ ] **`fmt` + `clippy` clean.**
      `cd elastos && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings`
      → 0 diffs, 0 warnings.

- [ ] **Perf baseline reproducible.**
      `ELASTOS_VZ_PERF_RUNS=2 bash scripts/measure-vz-baseline.sh`
      → writes `target/vz-baseline.json` with
      `schema_version: 2` and a non-`unknown` `git_sha`.

## Phase 6 unblockers (must be resolved before Day 1)

These were Phase-5 carry-forwards. Phase 6 cannot start
real microVM boot work without them.

### Unblocker 1: `components.json` darwin-arm64 release metadata

**Status check:**
```bash
python3 -c "import json; m=json.load(open('components.json')); print([k for k,v in m.get('external',{}).items() if any(p.get('os')=='darwin' for p in v.get('platforms',[]))])"
```

**Pass:** Every native binary listed in `components.json`
has a `platforms` entry with `os: darwin`, `arch: arm64`,
a real `url`, and a real `sha256`.

**Fail:** Phase 6 Day 1 work = source / cross-compile / sign
the missing binaries, populate the metadata, validate via
`local-carrier-setup-smoke.sh` (full lane, not dry-run).

> **Note.** This is the single biggest Phase-5 →
> Phase-6 unblocker. See
> `docs/vz-backend/PLAN.md` § "Pre-Work removed dishonest
> darwin entries" for the history.

### Unblocker 2: Self-hosted Mac CI runner activated

**Status check:**
```bash
gh variable get MAC_VZ_FULL_BOOT_ENABLED
# Should return "true" not "false" or unset.
gh workflow run _self-hosted-probe.yml  # heartbeat
```

**Pass:** Repo variable set to `true`, heartbeat workflow's
`probe-attempt` job runs successfully on the self-hosted
runner with labels `[self-hosted, macOS, ARM64, vz-capable]`.

**Fail:** Provision the runner per
`docs/vz-backend/SELF_HOSTED_RUNNER_SPEC.md`, then set the
repo variable.

> **Note.** Without this, the `mac-vz-full-boot` job in
> `mac-vz.yml` never executes — all CI is dry-run-only.

### Unblocker 3: First end-to-end full-boot smoke green

After Unblocker 1 + 2 are resolved:

**Status check:**
```bash
ELASTOS_VZ_SMOKE_FORCE_FULL=1 \
    bash scripts/local-carrier-setup-smoke.sh
# Should exit 0 after a real install + cargo build + run.
```

**Pass:** Smoke exits 0, no `dry-run` lines in output,
real `elastos serve` binary invoked.

**Fail:** Either the binaries are still missing / wrong
sha256 (back to Unblocker 1), or the runner doesn't have
the right host setup (back to Unblocker 2 +
`SELF_HOSTED_RUNNER_SPEC.md`).

## Phase 6 backlog (carry-forward from Phase 5)

These are *not* gates — they're the starting backlog. Each
should become a Day-by-Day item in `PHASE_6_PLAN.md`.

1. **Real-microVM perf measurement.** Expand
   `vz_perf_harness.rs` to include the
   `Supervisor::ensure_capsule` → real `LaunchMicroVm`
   path under both Vz and crosvm. The synthetic-launch
   metric stays (regression tripwire); the real-launch
   metric flips `notes.real_vz_boot_measured` to `true`.

2. **Bridge code + `TxExecutable` perf metrics.** Today's
   6 metrics cover supervisor + dispatch + capability. Add:
   `bridge_dispatch_rpc`, `tx_executable_full_cycle`,
   `vsock_rpc_round_trip` once those substrates settle.

3. **CI regression-detector.** Add a workflow that diffs
   `target/vz-baseline.json` against a committed
   `docs/vz-backend/baselines/vz-baseline.json` and fails
   if any p99 regressed by ≥20%. Same for crosvm.

4. **`PHASE_6_UNBLOCKERS.md`.** Adopt the convention
   suggested in `PHASE_5_RETROSPECTIVE.md` § What didn't
   go well. One file per phase, listing the hard
   dependencies that must be resolved before scope work
   begins.

5. **Phase-6 day-notes hygiene.** Continue the
   `PHASE_N_DAY_M_NOTES.md` discipline. Each day's note
   captures: scope, deliverables, scope deviation,
   carry-forward findings, quality gates.

6. **Cross-platform `update-cursor-settings` integration.**
   The skills under `~/.cursor/skills-cursor` are global;
   Phase 6 should consider committing a `.cursor/skills/`
   that captures the Vz-backend-specific tooling so new
   contributors get the right context out of the box.

## Phase 6 day-1 readiness signal

Phase 6 is **green-lit for Day 1** when:

1. All Phase 5 closeout gates above are checked.
2. All 3 Phase 6 unblockers above are resolved.
3. The Phase 6 lead has reviewed
   `PHASE_5_RETROSPECTIVE.md` § Carry-forward findings.
4. A `PHASE_6_PLAN.md` exists with at least the first 3
   days planned (the rest can be filled in as Phase 6
   progresses).

When all 4 are true: **start `PHASE_6_DAY_1_NOTES.md` and
go.**
