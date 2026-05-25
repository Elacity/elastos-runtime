# Vz-backend CI runbook

> **Phase 5 Day 5 + Day 6.** Operator runbook for the GitHub Actions workflows that gate Vz-backend changes. Covers how to interpret failures, how to drive a manual one-shot run, how to reproduce a CI failure locally, and how to operate the Day-6 self-hosted full-boot lane.

---

## 1. The workflows

| File | Purpose | Runs on | Trigger |
|---|---|---|---|
| `.github/workflows/ci.yml` | Linux baseline — fmt, clippy, tests, release build. Source of truth for full-workspace coverage. | `ubuntu-latest` | push to `main` / `sash/**` / `vz/**`; PR to `main`. |
| `.github/workflows/linux-untouched.yml` | Enforces the Linux-untouched guarantee from `docs/vz-backend/PLAN.md`. Runs `scripts/check-linux-untouched.sh` against the Phase 0 baseline commit (`a65dad3`). | `ubuntu-latest` | push to `sash/**` / `vz/**`; PR to `main`; `workflow_dispatch`. |
| `.github/workflows/mac-vz.yml` | Mac Apple-Silicon CI substrate. **Phase 5 Day 5** added three GitHub-hosted jobs (`mac-rust-tests`, `mac-shell-helpers`, `mac-smokes-dry-run`). **Phase 5 Day 6** added a fourth opt-in self-hosted job (`mac-vz-full-boot`). | `macos-latest` for jobs 1-3; `[self-hosted, macOS, ARM64, vz-capable]` for job 4. | push to `main` / `sash/**` / `vz/**`; PR to `main`; `workflow_dispatch`. |
| `.github/workflows/_self-hosted-probe.yml` | Heartbeat probe for the self-hosted runner. Two jobs: `probe-attempt` (gated, self-hosted) and `probe-fallback` (always, `ubuntu-latest`). **Phase 5 Day 6.** | `[self-hosted, macOS, ARM64, vz-capable]` + `ubuntu-latest` | `schedule: 0 0,6,12,18 * * *`; `workflow_dispatch`. |

All workflows share a `concurrency:` group so a rapid-fire push sequence auto-cancels the older run rather than queuing duplicate minutes.

---

## 2. Reading a failure

### 2.1 `mac-rust-tests` job

Three steps can fail:

| Step | Failure mode | Reproduce locally | Common root cause |
|---|---|---|---|
| `cargo fmt --check` | Code doesn't match `rustfmt` style. The action prints a unified diff. | `cd elastos && cargo fmt --all` then re-stage. | Forgot to run `just fmt` before committing. |
| `cargo clippy` | A new warning landed and `-D warnings` upgraded it to a hard error. The action prints the lint name (e.g. `clippy::needless_borrow`) + file:line. | `cd elastos && cargo clippy --workspace --all-targets -- -D warnings` | Recent dependency bump introduced a new lint level, OR your change has a real issue. |
| `cargo test (threads=1)` / `(threads=4)` | A test failed. The action prints `test result: FAILED. N passed; M failed`. Scroll up to find the specific `test … FAILED` line. | `cd elastos && cargo test -p elastos-server -p elastos-vz --tests -- --test-threads=1` (and `=4`) | (a) Real regression in the code path the test guards. (b) Concurrency bug — passes at `threads=1` but fails at `=4` indicates shared-state pollution. |

The whole `mac-rust-tests` job is mirrored exactly by `just ci-rust` locally. If `just ci-rust` passes on your Mac, CI is overwhelmingly likely to pass too. The remaining failure modes are environment drift (CI's `macos-latest` image vs your local) and rare time-sensitive tests.

### 2.2 `mac-shell-helpers` job

Two steps:

| Step | Asserts | Failure message |
|---|---:|---|
| `cross-platform.sh unit tests` | 44 | `FAIL <test_name>: <reason>` then `cross-platform.sh: N passed, M failed`. The action exits non-zero. |
| `runtime-cleanup.sh unit tests` | 5 | Same format, different prefix. |

A failure here means a bash-3.2 / BSD-utils incompatibility crept in. Reproduce locally with `just ci-shell`. The test file names are the same as the helper file names + `-test.sh`; the file:line in the failure points at the exact assertion that broke.

### 2.3 `mac-smokes-dry-run` job

Three steps, one per smoke. Each runs in its dry-run lane (the `cross_platform_in_ci` predicate auto-enables `ELASTOS_VZ_SMOKE_DRY_RUN=1` even without the explicit env-var; the workflow sets it explicitly anyway for operator-visible intent).

Each smoke prints exactly four lines on success:

```
[<smoke>] CI detected (GITHUB_ACTIONS or CI env set); auto-enabling ELASTOS_VZ_SMOKE_DRY_RUN=1
[<smoke>] dry-run mode: parse OK, helper sourced OK; exiting before <cost>
[<smoke>] dry-run: Vz host capability check passed (macOS 12+)
```

(The CI-detect echo line may not appear if `ELASTOS_VZ_SMOKE_DRY_RUN=1` was already set explicitly by the workflow env — that's fine, both paths converge on the same exit.)

A failure here is almost always one of:
- The smoke's bash syntax broke under bash 3.2 (the macOS default). Reproduce with `bash -n scripts/<smoke>.sh` locally.
- A new helper call in the smoke isn't present in `scripts/lib/cross-platform.sh`. The smoke would print `command not found: foo`.

Reproduce the whole job with `just ci-smokes-dry`.

---

## 3. Manual one-shot run from the Actions UI

All three workflows expose a `workflow_dispatch:` trigger. From the GitHub Actions UI:

1. Navigate to **Actions** → pick the workflow (e.g. `Mac Vz CI (Phase 5+ Apple Silicon)`).
2. Click **Run workflow** → select the branch → **Run workflow**.

Use this when:
- You've rebased onto an updated baseline and want to re-verify the Linux-untouched gate without pushing.
- A flake-suspect run failed and you want to retry without amending the commit.
- You want to gate a feature branch's mergeability before opening a PR.

---

## 3a. Self-hosted full-boot lane (Phase 5 Day 6)

### 3a.1 Smoke precedence

The three Mac smokes resolve their dry-run vs. full-run mode via a layered precedence table. Top of the table wins:

| Order | Setting | Outcome | Owner |
|---:|---|---|---|
| 1 | `ELASTOS_VZ_SMOKE_FORCE_FULL=1` | **Full run.** Overrides every layer below. | Self-hosted runner job (Day 6). |
| 2 | `ELASTOS_VZ_SMOKE_DRY_RUN=0` | **Full run.** Explicit operator opt-back-in. | Operator escape hatch. |
| 3 | `ELASTOS_VZ_SMOKE_DRY_RUN=1` | **Dry run.** Explicit operator opt-in. | Operator local debug. |
| 4 | CI auto-detect (`GITHUB_ACTIONS` or `CI` set) + `DRY_RUN` unset | **Dry run.** Day-5 default for the GitHub-hosted lane. | Day-5 default. |
| 5 | None of the above (local Mac dev) | **Full run.** | Default. |

The canonical implementation lives in `scripts/lib/cross-platform.sh::cross_platform_smoke_should_dry_run` and is pinned by three new assertions in `cross-platform-test.sh` (assertion count 41 → 44). The smokes today still carry inline FORCE_FULL + CI-auto-detect blocks for operator-visible echo lines; the helper documents the contract.

### 3a.2 Enabling the lane

Two switches must BOTH be on:

1. **Repository variable.** Settings → Secrets and variables → Actions → Variables → set `MAC_VZ_FULL_BOOT_ENABLED=true`. Default-off keeps the lane silent for public forks.
2. **Self-hosted runner.** A runner registered with the exact label set `[self-hosted, macOS, ARM64, vz-capable]` must be **Idle** in the Runners page. See [`SELF_HOSTED_RUNNER_SPEC.md`](./SELF_HOSTED_RUNNER_SPEC.md) for provisioning.

Phase 6 Day 5a shipped the one-command preflight that turns spec § 4 into a re-runnable bash script: `bash scripts/ci/setup-mac-runner.sh`. Run it on the Apple-Silicon Mac before § 4.3 (runner-agent install). The recipe verifies HW/OS prereqs, installs Rust toolchain if absent, delegates to `scripts/lib/components-json-verify.sh`, and prints the exact `gh variable set` + label set the operator types next. Exit codes are typed (0..4) per [`SELF_HOSTED_RUNNER_SPEC.md`](./SELF_HOSTED_RUNNER_SPEC.md) § 4.5.

When both switches are on, the next `mac-vz.yml` push/PR will schedule `mac-vz-full-boot` on the runner. The three smoke steps print `FORCE_FULL=1 — forcing full smoke run` and run the real Vz boot.

### 3a.3 The heartbeat probe

`_self-hosted-probe.yml` runs every 6 hours and on `workflow_dispatch`. It claims the same label set as `mac-vz-full-boot` plus emits a fallback job on `ubuntu-latest` that records the variable's current value. Use this to verify:

- A runner is online — `probe-attempt` completes in < 1 min with `PRESENT` printed.
- The lane is enabled — `probe-fallback` prints `MAC_VZ_FULL_BOOT_ENABLED=true`.

If `probe-attempt` queues past its 5-minute timeout, no matching runner is online. Either re-register the runner or disable the lane (unset the variable) until the runner is back.

### 3a.4 Kill switch

Two independent kill switches, both effective immediately:

| Action | Effect |
|---|---|
| Unset / set `MAC_VZ_FULL_BOOT_ENABLED` to anything other than `true`. | `if:` gate evaluates false; jobs do not schedule. |
| Remove the `vz-capable` label from the runner (or take the runner offline). | Job's `runs-on` no longer matches; jobs queue until timeout, then cancel. |

For routine maintenance prefer the variable; for emergency operator control prefer the runner label/offline.

---

## 4. Reproducing CI locally

| Goal | Recipe |
|---|---|
| Run the full mac-vz workflow locally | `just ci-mac` |
| Just the Rust gate | `just ci-rust` |
| Just the shell-helper tests | `just ci-shell` |
| Just the smokes (dry-run lane) | `just ci-smokes-dry` |
| Just the Linux-untouched gate | `just ci-linux-untouched` |

The `just` recipes are the local source of truth — the workflow's job steps are a verbatim copy. If you change one, update the other.

---

## 5. What CI does NOT cover yet

| Surface | Status | Tracked in |
|---|---|---|
| Real Vz microVM boot in CI on the **public** lane | Not covered. GitHub-hosted macOS runners don't reliably expose `Virtualization.framework` to nested processes. | Always — won't change. The Day-6 self-hosted lane is the substrate for real boots. |
| Real Vz microVM boot on the **self-hosted** lane | **Wired but dormant** (Phase 5 Day 6). Becomes active when an operator provisions a runner per [`SELF_HOSTED_RUNNER_SPEC.md`](./SELF_HOSTED_RUNNER_SPEC.md) and sets `MAC_VZ_FULL_BOOT_ENABLED=true`. | [`SELF_HOSTED_RUNNER_SPEC.md`](./SELF_HOSTED_RUNNER_SPEC.md). |
| Linux smoke runs (`local-carrier-setup-smoke.sh` etc.) full end-to-end on Linux | Not covered. CI runs only the dry-run lane; the full runs need a provisioned `~/.local/share/elastos` which CI doesn't have. | Phase 5 Day 8 follow-up — likely never automated; the local `just verify` recipe is the source of truth. |
| Performance / regression benchmarks | Not covered. | Phase 5 Day 7 deliverable. |
| Real `darwin-arm64` `components.json` release metadata + Carrier-backed install | **Structurally landed Phase 6 Days 2–4a.** Class-A/B/D/E green; Class-C vmlinux awaits Day-4b operator handoff to populate checksum + size. The smokes now run their full Vz boot path instead of pre-flight-skipping; bin-fetch fails with a typed error if Day-4b hasn't shipped a kernel yet. | [`PHASE_6_DAY_4_NOTES.md`](./PHASE_6_DAY_4_NOTES.md) § 4 Gate 4b-3 + Gate 4b-6 (Day-4b operator queue). |

---

## 6. CI cost protection

| Mechanism | What it does |
|---|---|
| `concurrency: group: …, cancel-in-progress: true` | Cancels older runs on the same branch when a new push lands. Rapid-fire commits don't pile up macOS-runner minutes. |
| `timeout-minutes: 30` on `mac-rust-tests` | Hard cap. Local wall-clock is ~30 s so the headroom is huge; the cap is the "something is wrong" tripwire. |
| `timeout-minutes: 5` on `mac-shell-helpers` | Wall-clock budget for the 41 + 5 assertions. They run in <2 s locally; 5 minutes is the "infinite loop in a helper" tripwire. |
| `timeout-minutes: 10` on `mac-smokes-dry-run` | All three smokes combined run in <2 s locally. 10 minutes is the "smoke went into a real run by accident" tripwire. |
| `Swatinem/rust-cache@v2` with a Mac-specific `prefix-key: mac-vz` | Caches the cargo target dir keyed by `Cargo.lock`. First-run cold-cache takes ~10 min; warm-cache <30 s. Distinct prefix prevents collision with the Linux `ci.yml` cache. |

Public-repo macOS minutes on GitHub Actions are free for open-source projects (10× cost ratio to Linux applies only to private repos). The cost-protection knobs above are still worth keeping as runaway-defence.

---

## 7. Anchors

- [`PHASE_5_PLAN.md`](PHASE_5_PLAN.md) § Day 5, § Day 6 — the original plans.
- [`PHASE_5_DAY_5_NOTES.md`](PHASE_5_DAY_5_NOTES.md) — Day-5 shipped surface + carry-forward findings.
- [`PHASE_5_DAY_6_NOTES.md`](PHASE_5_DAY_6_NOTES.md) — Day-6 shipped surface + carry-forward findings.
- [`PHASE_6_DAY_5_NOTES.md`](PHASE_6_DAY_5_NOTES.md) — Phase 6 Day 5a/5b split + operator handoff.
- [`SELF_HOSTED_RUNNER_SPEC.md`](SELF_HOSTED_RUNNER_SPEC.md) — provisioning contract for the Day-6 lane (Day 5a added § 4.5).
- [`PLAN.md`](PLAN.md) — overarching Vz-backend roadmap.
- `.github/workflows/mac-vz.yml` — the main workflow.
- `.github/workflows/_self-hosted-probe.yml` — the heartbeat probe.
- `scripts/ci/setup-mac-runner.sh` — **Phase 6 Day 5a** one-command preflight for the self-hosted runner.
- `justfile` § "Phase 5 Day 5 — CI-mirror recipes" — the local-mirror recipes.
