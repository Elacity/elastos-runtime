# Phase 5 Day 6 — Outcome Notes

> **Date:** 2026-05-25.
> **Branch:** local (push deferred per the day-by-day cadence).
> **Anchors:** [`PHASE_5_PLAN.md`](PHASE_5_PLAN.md) § Day 6, [`PHASE_5_DAY_5_NOTES.md`](PHASE_5_DAY_5_NOTES.md).
>
> **Headline:** Day-5 landed the **GitHub-hosted dry-run lane** for the Vz backend and explicitly deferred the real-Vz-boot lane to a self-hosted-runner spec. Day 6 ships exactly that spec — wired but dormant — plus the `FORCE_FULL=1` precedence layer that lets the self-hosted runner opt back into the full smokes after Day-5's CI auto-detect would have downgraded them to dry-run.

---

## 1. Scope-deviation note

The original Day-6 prompt (PHASE_5_PLAN.md § Day 6) was "Performance baseline document". That depends on having a reproducible CI surface that actually runs real Vz boots — which doesn't exist on GitHub-hosted runners. Day 6 ships the precondition (the self-hosted lane spec) instead; the perf-baseline scope shifts to Day 7.

---

## 2. What shipped

### 2.1 `ELASTOS_VZ_SMOKE_FORCE_FULL=1` precedence layer

New top-priority env var that **forces the full smoke run** regardless of every other gate (CI auto-detect, explicit `DRY_RUN=1`). Layered precedence (top wins):

| Order | Setting | Outcome | Owner |
|---:|---|---|---|
| 1 | `ELASTOS_VZ_SMOKE_FORCE_FULL=1` | Full run | Self-hosted runner job (Day 6). |
| 2 | `ELASTOS_VZ_SMOKE_DRY_RUN=0` | Full run | Operator escape hatch. |
| 3 | `ELASTOS_VZ_SMOKE_DRY_RUN=1` | Dry run | Operator local debug. |
| 4 | CI auto-detect + `DRY_RUN` unset | Dry run | Day 5. |
| 5 | None of the above | Full run | Default for local dev. |

**Wired into all three smokes:**
- `scripts/local-carrier-setup-smoke.sh`
- `scripts/home-frontdoor-smoke.sh`
- `scripts/chat-wasm-native-interop-smoke.sh`

Operator-visible echo line on activation:
```
[<smoke>] FORCE_FULL=1 — forcing full smoke run (overrides CI auto-detect)
```

### 2.2 Canonical precedence helper

`scripts/lib/cross-platform.sh` now exports `cross_platform_smoke_should_dry_run`, a pure-logic predicate that encapsulates the full precedence table above. The smokes today still carry the inline FORCE_FULL + CI-auto-detect blocks for operator-visible echo lines, but the helper is the **canonical contract** that the unit tests pin. A Phase-5-Day-8 follow-up can refactor the smokes to call the helper directly.

### 2.3 3 new shell-helper assertions (41 → 44)

`scripts/lib/cross-platform-test.sh` adds three new assertions covering the FORCE_FULL precedence layer:

1. `FORCE_FULL=1` alone → no dry-run.
2. `FORCE_FULL=1 + DRY_RUN=1` → FORCE_FULL wins.
3. `FORCE_FULL=1 + CI=true` → FORCE_FULL beats CI auto-detect.

Total assertions: **44 passing, 0 failing** (verified locally with `bash scripts/lib/cross-platform-test.sh`).

### 2.4 New `mac-vz-full-boot` CI job

`.github/workflows/mac-vz.yml` gains a fourth job: `mac-vz-full-boot`. Doubly gated:

- `if: ${{ vars.MAC_VZ_FULL_BOOT_ENABLED == 'true' }}` — repository variable opt-in (default off).
- `runs-on: [self-hosted, macOS, ARM64, vz-capable]` — exact label set.

When the gates are open the job:
1. Checks out the repo, installs the Rust toolchain.
2. Probes the Mac substrate (`sw_vers`, `uname -a`, `Virtualization.framework` presence).
3. Builds the release artefacts (`cargo build -p elastos-server -p elastos-vz --release`).
4. Runs the three Phase-5 smokes with `ELASTOS_VZ_SMOKE_FORCE_FULL=1`.

30-minute timeout. Cache prefix-key `mac-vz-self-hosted` (distinct from the GitHub-hosted cache).

### 2.5 New `_self-hosted-probe.yml` heartbeat workflow

`.github/workflows/_self-hosted-probe.yml` provides operator visibility into the self-hosted lane:

- **`probe-attempt`** — gated on the same repo variable + label set as `mac-vz-full-boot`. Claims the labels, prints `sw_vers + uname -a + Virtualization.framework` status, exits in < 1 min. If no matching runner is online, this queues until its 5-minute timeout, signalling "runner offline".
- **`probe-fallback`** — always runs on `ubuntu-latest`, records the variable's current value (`MAC_VZ_FULL_BOOT_ENABLED=<value>`). Gives the operator a scheduled audit trail for "when was the lane on?".

Schedule: `0 0,6,12,18 * * *` (four times per day) + `workflow_dispatch` for on-demand probes. Concurrency-cancel group `self-hosted-probe`.

### 2.6 New `SELF_HOSTED_RUNNER_SPEC.md`

`docs/vz-backend/SELF_HOSTED_RUNNER_SPEC.md` documents the operator contract end-to-end:

- Hardware + OS requirements (Apple Silicon, ≥ 16 GiB RAM, ≥ 100 GiB free disk, macOS 13+).
- Exact label set `[self-hosted, macOS, ARM64, vz-capable]`.
- Provisioning checklist (Rust, data dir, runner agent, repo variable).
- Security posture (hardware isolation, network segmentation, restricted repo variable, two-stage kill switch).
- Day-6 acceptance criteria (probe completes < 1 min, full-boot job completes within 30 min, dry-run lane unregressed, Linux-untouched green).
- Phase-6 deferrals (darwin-arm64 release metadata, signing/notarisation, perf baselines, multi-runner fleet, auto-recovery).

### 2.7 `CI_RUNBOOK.md` § 3a — Self-hosted lane

Comprehensive new section in the operator runbook:

- Precedence table (the same one in § 2.1 above).
- Two-switch enable flow (repo variable + runner registration).
- Heartbeat probe interpretation.
- Two-stage kill switch (variable for routine maintenance; label/offline for emergency).

Plus minor bumps elsewhere in the runbook:
- `cross-platform.sh` assertion count 41 → 44.
- Workflow table grew to include `_self-hosted-probe.yml`.
- "What CI does not cover" rewritten to distinguish "won't ever cover" (GitHub-hosted) from "wired but dormant" (self-hosted).

---

## 3. Operator benefits

- **Real Vz boots from CI become possible** once a self-hosted runner is provisioned. No more "works on my Mac" risk for Vz changes.
- **Public forks stay green** because the lane is default-off. PR contributors don't see queue-timeout failures for jobs they can't run.
- **Two independent kill switches** (variable + label) means routine maintenance and emergency response use different controls.
- **Audit trail in the Actions UI** via `probe-fallback`'s scheduled run history — operators can answer "when did we enable the lane?" without reading commit history.
- **Local-CI alignment preserved** — every Day-5 `just ci-*` recipe still mirrors the GitHub-hosted lane verbatim. Day-6 doesn't add new `just` recipes because the self-hosted lane can only run on real Apple-Silicon hardware (which is the local Mac itself for developers).

---

## 4. Carry-forward findings

1. **Smokes still carry inline FORCE_FULL + CI-auto-detect blocks** even though the canonical precedence helper now exists in `cross-platform.sh`. A Phase-5-Day-8 follow-up should refactor the smokes to call `cross_platform_smoke_should_dry_run` and let the helper own all the echo lines (removes duplication × 3).
2. **`_self-hosted-probe.yml`'s 6-hour cron costs 24 ubuntu-minutes/day** — negligible for an open-source repo but worth knowing. If the cron's signal becomes noisy, drop to twice-daily.
3. **The repo variable `MAC_VZ_FULL_BOOT_ENABLED` has no validation** — setting it to e.g. `True` (capital T) won't enable the lane because the `if:` expression uses strict equality. Documented in the spec but a future hardening could broaden to a case-insensitive regex.
4. **No retry logic on the full-boot smokes.** If a single Vz boot flakes, the whole `mac-vz-full-boot` job fails. The Phase-4 typed-error surface gives us structured failure data so retries can be implemented honestly; deferred to Phase 6 or a Phase-5-Day-7 add-on.
5. **`SELF_HOSTED_RUNNER_SPEC.md` § 4.2 needs a concrete `just provision-mac-runner` recipe** once Phase 6 lands the darwin-arm64 release artefacts. Today the spec asks the operator to hand-bootstrap the data dir.

---

## 5. Runbook addendum

**To enable the Day-6 lane (operator workflow):**

```sh
# 1. Provision a Mac per docs/vz-backend/SELF_HOSTED_RUNNER_SPEC.md § 4.
# 2. Register the runner with labels:
#    self-hosted,macOS,ARM64,vz-capable
# 3. Set repository variable in GitHub UI:
#    Settings → Secrets and variables → Actions → Variables → New
#    Name:  MAC_VZ_FULL_BOOT_ENABLED
#    Value: true
# 4. Trigger the probe to confirm:
#    Actions → "Self-hosted Mac runner probe" → Run workflow
# 5. The next mac-vz.yml push/PR will schedule mac-vz-full-boot.
```

**To kill the lane (emergency):**

```sh
# Option A (routine): unset the repo variable.
# Option B (emergency): remove the `vz-capable` label from the runner
#                       OR take the runner offline.
```

Both options stop new jobs scheduling immediately.

---

## 6. Quality gates

- [x] `cargo fmt --all -- --check` — no diff.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- [x] `cargo test -p elastos-server -p elastos-vz --tests -- --test-threads=1` — green.
- [x] `cargo test -p elastos-server -p elastos-vz --tests -- --test-threads=4` — green.
- [x] `bash scripts/lib/cross-platform-test.sh` — 44 passed, 0 failed.
- [x] `bash scripts/lib/runtime-cleanup-test.sh` — 5 passed, 0 failed.
- [x] Three smokes with `ELASTOS_VZ_SMOKE_DRY_RUN=1` — pass.
- [x] Three smokes with `ELASTOS_VZ_SMOKE_FORCE_FULL=1 + CI=true` — full-run path entered (Mac pre-flight banner surfaces, expected for current darwin-arm64 components.json state).
- [x] `scripts/check-linux-untouched.sh bcf5a0a` — green.
- [x] YAML structural validation on `mac-vz.yml` (4 jobs, 3 FORCE_FULL=1 env lines, no tabs) — pass.
- [x] YAML structural validation on `_self-hosted-probe.yml` (2 jobs, gated probe-attempt + always-on probe-fallback) — pass.

---

## 7. Files changed (summary)

| Change | File |
|---|---|
| New | `.github/workflows/_self-hosted-probe.yml` |
| New | `docs/vz-backend/SELF_HOSTED_RUNNER_SPEC.md` |
| New | `docs/vz-backend/PHASE_5_DAY_6_NOTES.md` (this file) |
| Modified | `.github/workflows/mac-vz.yml` (added `mac-vz-full-boot` job; assertion-count bump) |
| Modified | `scripts/local-carrier-setup-smoke.sh` (FORCE_FULL block) |
| Modified | `scripts/home-frontdoor-smoke.sh` (FORCE_FULL block) |
| Modified | `scripts/chat-wasm-native-interop-smoke.sh` (FORCE_FULL block) |
| Modified | `scripts/lib/cross-platform.sh` (canonical precedence helper) |
| Modified | `scripts/lib/cross-platform-test.sh` (3 new assertions) |
| Modified | `docs/vz-backend/CI_RUNBOOK.md` (§ 3a self-hosted lane + assertion-count bump + workflow-table update) |
| Modified | `docs/vz-backend/PHASE_5_PLAN.md` (status bump + Day-6 outcome header + scope-deviation note) |
| Modified | `docs/vz-backend/PLAN.md` (status row update) |
| Modified | `docs/MAC.md` (capability matrix bump) |

No Rust code changes. Day 6 is pure ops surface + shell substrate.
