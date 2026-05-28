# Phase 5 — Day 5 — macOS GitHub Actions runner: CI substrate for Days 1–4

> **Status:** Complete. One commit, push deferred.
>
> **Plan reference:** [`PHASE_5_PLAN.md` § Day 5](PHASE_5_PLAN.md#day-5--macos-github-actions-runner-ci-substrate-for-days-14-46-h).
>
> **Anchors:** [`CI_RUNBOOK.md`](CI_RUNBOOK.md) (the operator-facing companion to this notes doc), [`PHASE_5_DAY_1_NOTES.md`](PHASE_5_DAY_1_NOTES.md) through [`PHASE_5_DAY_4_NOTES.md`](PHASE_5_DAY_4_NOTES.md).

---

## 1. What shipped

### 1.1 `cross_platform_in_ci` predicate (`scripts/lib/cross-platform.sh`)

Bash-3.2-clean function. Returns 0 (true) when EITHER `$GITHUB_ACTIONS` is set OR `$CI` matches one of the truthy tokens (`true` / `TRUE` / `1` / `yes` / `on` etc.). Returns non-zero (false) when both are unset/empty.

GitHub Actions sets both `GITHUB_ACTIONS=true` AND `CI=true`, so the dual recognition is belt-and-braces; the second branch matters only for projects that mirror this workflow into other CI providers (CircleCI, Travis, GitLab).

**Why a predicate, not just inlining the env check.** Inlining the check in each of the three smokes would have produced three copies of "`[[ -n "${GITHUB_ACTIONS:-}" || …` — duplication that drifts on the first env-var addition. Centralising the predicate also lets the unit-test file lock in the dual-recognition contract.

### 1.2 4 new assertions in `scripts/lib/cross-platform-test.sh` (37 → 41)

| Case | What it locks in |
|---|---|
| Both env vars unset → returns 1 | The "no false positives" branch — local Mac development is correctly identified as not-in-CI. |
| `GITHUB_ACTIONS=true` only → returns 0 | The primary GitHub Actions recognition. |
| `CI=true` only → returns 0 | The third-party-CI recognition. |
| Both env vars set → returns 0 | The actual GitHub Actions runtime case. Locks in dual recognition. |

Each assertion runs in an isolated sub-shell (`(unset GITHUB_ACTIONS CI; …)`) so the test file itself running in CI doesn't leak its env into the assertions.

### 1.3 CI auto-dry-run wired into the three Mac smokes

The same one-line block added near the top of each smoke (after the `cross-platform.sh` source):

```bash
if [[ -z "${ELASTOS_VZ_SMOKE_DRY_RUN:-}" ]] && cross_platform_in_ci; then
    echo "[<smoke>] CI detected (GITHUB_ACTIONS or CI env set); auto-enabling ELASTOS_VZ_SMOKE_DRY_RUN=1"
    export ELASTOS_VZ_SMOKE_DRY_RUN=1
fi
```

**Precedence contract:** the explicit `ELASTOS_VZ_SMOKE_DRY_RUN=0` setting always wins (the `-z` guard ensures the auto-detect only fires when the env is otherwise silent). Verified by `CI=true ELASTOS_VZ_SMOKE_DRY_RUN=0 bash scripts/local-carrier-setup-smoke.sh` — the smoke proceeds past the dry-run check into the Mac pre-flight (where it visible-skips on the existing Phase-6-prereq `components.json` darwin metadata gap).

The auto-dry-run keeps PR checks quiet by turning the "smoke would visible-skip noisily because CI doesn't have a `~/.local/share/elastos`" case into a clean dry-run pass. Operators with self-hosted runners that DO provision a data dir set `ELASTOS_VZ_SMOKE_DRY_RUN=0` explicitly to opt back into the full smoke.

### 1.4 `.github/workflows/mac-vz.yml` — new macOS Apple-Silicon CI workflow

Three jobs running on `macos-latest` (currently Apple Silicon ARM64):

| Job | Steps | Timeout |
|---|---|---|
| `mac-rust-tests` | `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test -p elastos-server -p elastos-vz --tests -- --test-threads=1`, same at `=4`. | 30 min |
| `mac-shell-helpers` | `bash scripts/lib/cross-platform-test.sh` (41), `bash scripts/lib/runtime-cleanup-test.sh` (5). | 5 min |
| `mac-smokes-dry-run` | The three Phase-5 smokes with `ELASTOS_VZ_SMOKE_DRY_RUN=1` set explicitly (the CI auto-detect would also catch them; the explicit env makes operator intent visible in the Actions UI). | 10 min |

**Cost-protection knobs:**
- `concurrency: { group: mac-vz-${{ github.ref }}, cancel-in-progress: true }` — rapid-fire pushes auto-cancel the older run.
- Three timeout caps per job — local wall-clock for the full suite is ~30 s, so the headroom is the "something is wrong" tripwire, not a real budget.
- `Swatinem/rust-cache@v2` keyed by `Cargo.lock` with a `prefix-key: mac-vz` so the Mac cache doesn't collide with the Linux `ci.yml` cache. First-run cold-cache ~10 min; warm-cache <30 s.

**Trigger surface:** `push` to `main` / `sash/**` / `vz/**`; `pull_request` to `main`; `workflow_dispatch` for manual one-shot runs from the Actions UI.

**Scope intentionally limited to `elastos-server` + `elastos-vz`.** The Linux `ci.yml` is the source of truth for full-workspace test coverage; the Mac runner doesn't need to re-pay to test every Linux-only crate. If Phase 5 Day 7 adds Mac-relevant tests outside these two crates, we widen the scope explicitly.

### 1.5 `.github/workflows/linux-untouched.yml` — added `workflow_dispatch:` trigger

Existing workflow already gated `scripts/check-linux-untouched.sh a65dad3` on every push and PR. Day 5 adds the manual-one-shot trigger so operators can re-run the gate after rebasing without pushing a new commit (matches the new mac-vz workflow's trigger surface).

### 1.6 `justfile` — five new `ci-*` recipes

```text
ci-mac                  # full mac-vz CI sequence locally (ci-rust + ci-shell + ci-smokes-dry)
ci-rust                 # mirror mac-rust-tests
ci-shell                # mirror mac-shell-helpers
ci-smokes-dry           # mirror mac-smokes-dry-run
ci-linux-untouched      # local-side gate against the Phase 0 baseline a65dad3
```

The workflow's job names match the recipe names verbatim for grep-ability. The recipes are the **local source of truth** for what the workflow runs — keep them in sync.

### 1.7 `docs/vz-backend/CI_RUNBOOK.md` — operator runbook

Covers the three workflows, how to read each failure mode, how to drive a `workflow_dispatch` run, the local-reproduction recipes, the explicit list of what CI does NOT cover (real Vz boot, Linux full-end-to-end smokes, perf benchmarks, real darwin-arm64 metadata), and the cost-protection knobs. Linked from `PHASE_5_DAY_5_NOTES.md`, `PHASE_5_PLAN.md`, `PLAN.md`.

---

## 2. Verification on this Mac

| Gate | Outcome |
|---|---|
| `bash scripts/lib/cross-platform-test.sh` | 41 passed, 0 failed (was 37 before Day 5). |
| `bash scripts/lib/runtime-cleanup-test.sh` | 5 passed, 0 failed. |
| `CI=true bash scripts/local-carrier-setup-smoke.sh` | Auto-dry-run fires; smoke exits 0 in <500 ms. |
| `GITHUB_ACTIONS=true bash scripts/home-frontdoor-smoke.sh` | Auto-dry-run fires; smoke exits 0 in <500 ms. |
| `CI=true bash scripts/chat-wasm-native-interop-smoke.sh` | Auto-dry-run fires; smoke exits 0 in <500 ms. |
| `CI=true ELASTOS_VZ_SMOKE_DRY_RUN=0 bash scripts/local-carrier-setup-smoke.sh` | Explicit `=0` wins; smoke proceeds past dry-run into Mac pre-flight (visible-skips on the existing Phase-6-prereq metadata gap, as designed). |
| `just ci-shell` | 41 + 5 = 46 assertions pass. |
| `just ci-smokes-dry` | All three smokes pass in <2 s wall-clock total. |
| `just ci-rust` | (skipped during this notes-writing iteration; covered by the global Day-5 gate run below). |

---

## 3. Carry-forward findings (no scope expansion)

### 3.1 Real Vz microVM boot is NOT yet covered in CI

GitHub-hosted macOS runners (`macos-latest` = `macos-15` at time of writing) **do** carry the `Virtualization.framework` binary in `/System/Library/Frameworks/`, but the nested-virt support varies by hypervisor: when the runner itself is virtualised (the common case), inner `VZVirtualMachineConfiguration::validate()` calls can return `VZErrorNotSupported` for kernel boot.

The Phase 5 Day 5 deliverable is the **CI substrate** — fmt, clippy, tests, smokes-dry-run. Phase 5 Day 6+ deliverable is the self-hosted runner spec for real boot. We carry this forward explicitly; the `CI_RUNBOOK.md` § 5 makes the gap visible to operators.

### 3.2 The `linux-untouched.yml` baseline drifts

The existing workflow uses baseline `a65dad3` (Phase 0). My local Day-4 work was verified against baseline `bcf5a0a` (a different snapshot). Both pass against the Phase 4/5 commits because the four protected crates haven't been touched, but the dual baselines are a minor consistency wart.

Not in scope to fix in Day 5 (the workflow's baseline is a deliberate choice — `a65dad3` is more conservative). Documented here so it's not forgotten.

### 3.3 The `ci.yml` workflow remains Linux-only

Day 5 deliberately doesn't extend `ci.yml` to also run on macOS. Two reasons:
1. The existing `ci.yml` runs full-workspace tests (including Linux-specific crates); the Mac runner doesn't need that breadth.
2. Splitting the Mac substrate into its own workflow makes failures self-describing in the Actions UI ("Mac Vz CI failed" vs "CI failed — but which platform?").

If a future maintainer wants a single unified workflow, the recipe pattern (each job mirrored by a `just ci-*` recipe) is the migration path: copy the Mac jobs into `ci.yml` as a `runs-on: macos-latest` matrix branch and delete `mac-vz.yml`. Day 5 doesn't pre-commit to either shape.

### 3.4 `concurrency: cancel-in-progress: true` may surprise operators mid-merge

If two PRs against `main` get merged within ~30 s of each other, the second merge can cancel the first's in-flight check. The risk is real but small (we've never had two simultaneous merges in this repo's history) and the benefit (no queued macOS minutes) outweighs the risk.

Future fix if needed: drop `cancel-in-progress: true` for the `main` branch via a conditional `concurrency:` group. Out of scope for Day 5.

---

## 4. Operator runbook addendum (CI-specific)

See [`CI_RUNBOOK.md`](CI_RUNBOOK.md) for the full operator-facing runbook. Highlights:

- **Reproduce any CI failure locally:** `just ci-mac` runs every step the `mac-vz.yml` workflow runs. If `just ci-mac` passes locally, CI is overwhelmingly likely to pass too.
- **Force a re-run without pushing:** use the `workflow_dispatch:` trigger from the Actions UI. Available on all three Vz-related workflows.
- **What CI does NOT cover:** real Vz boot, full end-to-end Linux smokes, perf benchmarks, real `darwin-arm64` `components.json` metadata. The smokes visible-skip honestly on the last item via `cross_platform_assert_native_binary_release_metadata`.

---

## 5. Quality gates

| Gate | Status |
|---|---|
| `cargo fmt --check` (workspace) | ✓ |
| `cargo clippy --workspace --all-targets -- -D warnings` | ✓ |
| `cargo test -p elastos-server -p elastos-vz --tests` under `--test-threads=1` | ✓ |
| `cargo test -p elastos-server -p elastos-vz --tests` under `--test-threads=4` | ✓ |
| `bash scripts/lib/cross-platform-test.sh` (now 41 assertions, was 37) | ✓ |
| `bash scripts/lib/runtime-cleanup-test.sh` (5 assertions) | ✓ |
| `ELASTOS_VZ_SMOKE_DRY_RUN=1 scripts/local-carrier-setup-smoke.sh` | ✓ |
| `ELASTOS_VZ_SMOKE_DRY_RUN=1 scripts/home-frontdoor-smoke.sh` | ✓ |
| `ELASTOS_VZ_SMOKE_DRY_RUN=1 scripts/chat-wasm-native-interop-smoke.sh` | ✓ |
| `CI=true scripts/<smoke>.sh` (no explicit DRY_RUN env) auto-fires the dry-run path | ✓ (all three) |
| `CI=true ELASTOS_VZ_SMOKE_DRY_RUN=0 scripts/<smoke>.sh` proceeds past the dry-run check (explicit `=0` wins) | ✓ |
| `just ci-shell` | ✓ |
| `just ci-smokes-dry` | ✓ |
| `just --list` includes `ci-mac`, `ci-rust`, `ci-shell`, `ci-smokes-dry`, `ci-linux-untouched` | ✓ |
| `scripts/check-linux-untouched.sh bcf5a0a` | ✓ |
| YAML syntax of `mac-vz.yml` + `linux-untouched.yml` | ✓ (`python3 -c 'import yaml; yaml.safe_load(open(p))'` for each) |
| Single commit (push deferred) | ✓ |

---

## 6. Next: Day 6

Phase 5 Day 6 designs the **self-hosted runner spec** for real Vz microVM boot in CI. That's where the smokes graduate from dry-run to full end-to-end with a kernel + rootfs + Carrier-bridge boot. The 10/10 prompt for Day 6 is the next deliverable; details live in [`PHASE_5_PLAN.md`](PHASE_5_PLAN.md) § Day 6.
