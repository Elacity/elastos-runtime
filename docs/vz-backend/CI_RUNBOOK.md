# Vz-backend CI runbook

> **Phase 5 Day 5.** Operator runbook for the GitHub Actions workflows that gate Vz-backend changes. Covers how to interpret failures, how to drive a manual one-shot run, and how to reproduce a CI failure locally.

---

## 1. The workflows

| File | Purpose | Runs on | Trigger |
|---|---|---|---|
| `.github/workflows/ci.yml` | Linux baseline — fmt, clippy, tests, release build. Source of truth for full-workspace coverage. | `ubuntu-latest` | push to `main` / `sash/**` / `vz/**`; PR to `main`. |
| `.github/workflows/linux-untouched.yml` | Enforces the Linux-untouched guarantee from `docs/vz-backend/PLAN.md`. Runs `scripts/check-linux-untouched.sh` against the Phase 0 baseline commit (`a65dad3`). | `ubuntu-latest` | push to `sash/**` / `vz/**`; PR to `main`; `workflow_dispatch`. |
| `.github/workflows/mac-vz.yml` | Mac Apple-Silicon CI substrate. Three jobs: `mac-rust-tests`, `mac-shell-helpers`, `mac-smokes-dry-run`. **Phase 5 Day 5.** | `macos-latest` | push to `main` / `sash/**` / `vz/**`; PR to `main`; `workflow_dispatch`. |

All three workflows share a `concurrency:` group (`mac-vz-${{ github.ref }}` etc.) so a rapid-fire push sequence auto-cancels the older run rather than queuing duplicate minutes.

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
| `cross-platform.sh unit tests` | 41 | `FAIL <test_name>: <reason>` then `cross-platform.sh: N passed, M failed`. The action exits non-zero. |
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
| Real Vz microVM boot in CI | Not covered. GitHub-hosted macOS runners don't reliably expose `Virtualization.framework` to nested processes. | Phase 5 Day 6+ self-hosted-runner spec (out of scope for Day 5). |
| Linux smoke runs (`local-carrier-setup-smoke.sh` etc.) full end-to-end on Linux | Not covered. CI runs only the dry-run lane; the full runs need a provisioned `~/.local/share/elastos` which CI doesn't have. | Phase 5 Day 8 follow-up — likely never automated; the local `just verify` recipe is the source of truth. |
| Performance / regression benchmarks | Not covered. | Phase 5 Day 7 deliverable. |
| Real `darwin-arm64` `components.json` release metadata + Carrier-backed install | Not covered. | Phase 6 — the smokes correctly visible-skip on this today via `cross_platform_assert_native_binary_release_metadata`. |

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

- [`PHASE_5_PLAN.md`](PHASE_5_PLAN.md) § Day 5 — the original plan.
- [`PHASE_5_DAY_5_NOTES.md`](PHASE_5_DAY_5_NOTES.md) — what shipped + the carry-forward findings.
- [`PLAN.md`](PLAN.md) — overarching Vz-backend roadmap.
- `.github/workflows/mac-vz.yml` — the workflow itself.
- `justfile` § "Phase 5 Day 5 — CI-mirror recipes" — the local-mirror recipes.
