# Phase 6 Day 5 — Self-hosted Mac runner activation (5a scaffolding)

**Phase**: 6 (macOS native-binary surface)
**Day**: 5 (split: 5a agent-shipped, 5b operator-shipped)
**Date**: 2026-05-25
**Status**: 5a complete; 5b operator handoff documented (see § 4)
**Predecessor**: [`PHASE_6_DAY_4_NOTES.md`](./PHASE_6_DAY_4_NOTES.md)
**Successor**: Day 6 — First end-to-end FORCE_FULL smoke green on self-hosted

---

## 1. Scope deviation from plan — Day 5 split into 5a + 5b

The original Day-5 prompt described eight gates that all assume an
operator has **physical access to an Apple-Silicon Mac**, runs the
GitHub Actions registration flow, sets the `MAC_VZ_FULL_BOOT_ENABLED`
repository variable, and verifies the first `_self-hosted-probe.yml` +
`mac-vz-full-boot` runs go green within 24 h. None of those steps can
close inside an agent session — they require a physical machine, a
short-lived GitHub registration token, and repo-admin credentials.

This mirrors **the exact same precedent set by Day 4** ([`PHASE_6_DAY_4_NOTES.md`](./PHASE_6_DAY_4_NOTES.md)
§ 1): the agent ships the reproducible scaffolding; the operator runs
it.

**Day 5a (this commit — agent-shipped):**

- A single re-runnable bash recipe ([`scripts/ci/setup-mac-runner.sh`](../../scripts/ci/setup-mac-runner.sh))
  that turns spec § 4 into one command the operator runs on the Mac
  before § 4.3's runner-agent install. The recipe verifies HW/OS
  prereqs (arm64, macOS ≥ 13, RAM ≥ 16 GiB, free disk ≥ 100 GiB),
  installs the Rust toolchain if absent, delegates to
  [`scripts/lib/components-json-verify.sh`](../../scripts/lib/components-json-verify.sh),
  probes the Day-4b vmlinux artefact, and prints the exact `gh
  variable set` + label set the operator types next. Typed exit codes
  (0..4) let the operator wire `&&` chains around it.
- Spec ([`SELF_HOSTED_RUNNER_SPEC.md`](./SELF_HOSTED_RUNNER_SPEC.md))
  promoted from "Phase 5 Day 6 deliverable, not yet provisioned" to
  "Phase 5 Day 6 → Phase 6 Day 5a — recipe available, pre-flight no
  longer skips". § 4.5 (new) documents the recipe; § 7 (status table)
  updated to reflect Days 2–4a outcomes; references updated to point
  at the new recipe + Day-4a/4b/5a notes.
- Runbook ([`CI_RUNBOOK.md`](./CI_RUNBOOK.md)) cross-references the
  new recipe in § 3a.2 and updates the § 5 "what CI does NOT cover"
  status row for darwin-arm64 metadata from "not covered" to
  "structurally landed Phase 6 Days 2–4a".

**Day 5b (operator-shipped — separate operator wall-clock):**

- Run `bash scripts/ci/setup-mac-runner.sh` on the physical Apple-Silicon
  Mac.
- Register the GitHub Actions runner agent with the exact 4-label set
  (`self-hosted,macOS,ARM64,vz-capable`) per spec § 4.3.
- Set the repository variable: `gh variable set MAC_VZ_FULL_BOOT_ENABLED
  --body true`.
- Trigger `_self-hosted-probe.yml` manually and confirm the
  `probe-attempt` job completes in < 1 min.
- Optionally trigger `mac-vz.yml::mac-vz-full-boot` via
  `workflow_dispatch` for a first real-substrate run. (Day 6 makes
  this gate-binding; Day 5 just verifies the runner is wired.)

**Why the split is honest, not a regression.** The original Day-5
prompt's gates 1–4 (procure HW, install runner agent, set repo var,
probe success) are operator-only by construction. The prompt's gates
5–8 (first job run + spec/runbook updates) split cleanly: spec/runbook
updates are agent-side (this commit); the first job run is operator-side
(after registration). Day 5a + Day 5b together cover all 8 original
gates — the split just stops pretending an agent can do both halves.

---

## 2. Concrete changes (Day 5a — agent-shipped)

### 2.1 `scripts/ci/setup-mac-runner.sh` (new, 215 LoC)

One-command preflight + provisioning script. Layout:

| § | Block | Purpose |
|---|---|---|
| 1 | HW/OS preflight | arm64 + macOS ≥ 13 + RAM ≥ 16 GiB + free-disk ≥ 100 GiB. Floors taken verbatim from [`SELF_HOSTED_RUNNER_SPEC.md`](./SELF_HOSTED_RUNNER_SPEC.md) § 2. |
| 2 | Toolchain | Xcode CLT (async install if absent — exits 2 with re-run instruction); `rustup` stable; `rustup component add clippy rustfmt`. |
| 3 | Vz framework | `/System/Library/Frameworks/Virtualization.framework` presence check. |
| 4 | components.json | Delegate to [`scripts/lib/components-json-verify.sh`](../../scripts/lib/components-json-verify.sh). Single source of truth — no duplicated invariants. |
| 5 | Day-4b artefact probe | Look for the operator-built vmlinux Image at `$XDG_DATA_HOME/elastos/bin/vmlinux`; verify its sha256 against `components.json` when populated; log "Day-4b operator handoff pending" + exact build-recipe path when absent. Plus list Class-E helper cache state (kubo / cloudflared / llama-server). Informational only — does NOT exit non-zero on missing artefacts (the smokes themselves report typed errors). |
| 6 | Operator handoff | Multi-line `╔═══` summary printing the exact `gh variable set` + runner registration + kill-switch commands. The 4-label set is embedded as a literal so the operator can copy-paste with zero typo risk. |

**Idempotent.** Re-running on a partially-provisioned machine completes
the gaps and reports what was already done. No destructive operations
anywhere in the recipe.

**Typed exit codes:**

| Exit | Meaning |
|---:|---|
| 0 | All checks green; provisioning ready for runner-agent install (spec § 4.3) + variable flip (§ 4.4). |
| 1 | HW/OS prerequisite failed (Intel Mac, macOS < 13, RAM/disk floor). |
| 2 | Toolchain install failed. |
| 3 | `Virtualization.framework` absent. |
| 4 | `components.json` verifier failed. |

**Live-tested today on the dev Mac:**

```text
── 1. HW/OS preflight ────────────────────────────────────────
[setup-mac-runner] architecture: arm64 (Apple Silicon) ✓
[setup-mac-runner] macOS: 26.4.1 (>= 13) ✓
[setup-mac-runner] RAM: 48 GB (>= 16) ✓
[setup-mac-runner] free disk on $HOME: 614 GB (>= 100) ✓
── 2. Toolchain ────────────────────────────────────────
[setup-mac-runner] Xcode CLT: /Library/Developer/CommandLineTools ✓
[setup-mac-runner] rustc: rustc 1.89.0 (29483883e 2025-08-04) ✓
── 3. Virtualization.framework ────────────────────────────────────────
[setup-mac-runner] Virtualization.framework PRESENT ✓
── 4. components.json invariants ────────────────────────────────────────
[components-json-verify] OK
  Class A (host binaries):    7/7 green
  Class B (microVM bundles):  1/1 green (D.2.a share-bundle invariant enforced)
  Class C (kernel):           1/1 green (structural; checksum populated by Day-4b operator handoff)
  Class D (linux-only):       1/1 green (darwin absent as required)
  Class E (3rd-party):        3/3 green (real url + checksum)
  Capsules projection:        10 entries include 'aarch64-darwin'
[setup-mac-runner] components.json verifier: green ✓
── 5. Day-4b artefact probe (informational) ────────────────────────────────────────
[setup-mac-runner] WARN: vmlinux not yet present at /Users/sash/.local/share/elastos/bin/vmlinux
[setup-mac-runner] WARN:   → Day-4b operator handoff pending. Run:
[setup-mac-runner] WARN:       bash /Users/sash/code/elastos-runtime/scripts/build-vmlinux-arm64.sh
[…]
── 6. Operator handoff (Day 5b — runner registration) ────────────────────────────────────────
╔═══════════════════════════════════════════════════════════════════════════
║ Provisioning preflight GREEN. Continue with Day-5b operator handoff.
[…]
```

Exit code = 0. ✅ The recipe correctly identifies that the dev Mac is
provision-ready except for the Day-4b artefact (matches the on-disk
state — Day-4b is intentionally not yet run).

**Why no `just provision-mac-runner` recipe.** The original spec § 4.2
floated a `just` target. Today the recipe is a single bash invocation
with no parameters; a `just` shim would be 1:1 wrapping that adds no
value but adds a dependency on having `just` installed on the runner
before the recipe runs (chicken-and-egg). Phase 7 carry-forward: if
`just` becomes a runner prereq for other reasons, fold the recipe in
then.

### 2.2 `docs/vz-backend/SELF_HOSTED_RUNNER_SPEC.md` (updated, +37 / −10)

Five surgical edits:

1. **Header banner** — "Phase 5 Day 6 deliverable, not yet provisioned"
   → "Phase 5 Day 6 → Phase 6 Day 5a — recipe available; pre-flight no
   longer visible-skips post-Days 2–3". Honest status update.
2. **§ 4.2 (data-dir provisioning)** — replaced the "hand-bootstrap"
   stub with a one-line invocation of the new recipe + a "today's
   reality post-Day-4a" paragraph that documents the current Class-A/B/D/E
   green state + Class-C operator-handoff pending state. Clear delineation
   of "what the smokes do today" vs "what changes when Day-4b ships".
3. **§ 4.5 (new section)** — recipe entry-point, typed exit-code table,
   explicit "what the recipe does NOT do and why" so the operator
   doesn't expect it to register the runner agent for them.
4. **§ 7 (status table)** — two rows refreshed:
   - "darwin-arm64 release metadata" → "Phase 6 Days 2–4a structurally
     completed"
   - "Code signing + notarisation" → "Phase 6 Day 4a recipes shipped;
     Day 4b operator-side execution"
5. **§ 8 (references)** — added the new recipe + Day-4 + Day-5 notes.

### 2.3 `docs/vz-backend/CI_RUNBOOK.md` (updated, +5 / −2)

Three surgical edits:

1. **§ 3a.2 (enabling the lane)** — appended a paragraph cross-referencing
   the new recipe + reiterating the typed exit codes + linking back to
   spec § 4.5 for the full detail.
2. **§ 5 (status table — "darwin-arm64 release metadata" row)** —
   "Not covered" → "structurally landed Phase 6 Days 2–4a; Class-C
   awaits Day-4b operator handoff". Tracked-in column points at
   PHASE_6_DAY_4_NOTES.md § 4 Gate 4b-3 / 4b-6 for the operator queue.
3. **§ 7 (anchors)** — added the new recipe + Day-5 notes.

### 2.4 What did NOT change (deliberate parsimony)

- **`.github/workflows/mac-vz.yml`** — already shipped in Phase 5 Day 6;
  Day-5a verification confirmed the `mac-vz-full-boot` job + double-gate
  (`vars.MAC_VZ_FULL_BOOT_ENABLED` + 4-label runs-on) still match the
  current spec.
- **`.github/workflows/_self-hosted-probe.yml`** — same; no changes
  needed for Day 5a's scaffolding.
- **`components.json`** — Day-4a's `vmlinux` darwin-arm64 stub is the
  Day-5a contract too; no additional populating in this commit. Day 4b
  remains the operator-side gate that fills the checksum + size.
- **`scripts/lib/components-json-verify.sh`** — already covers all
  invariants Day-5a needs; the setup script delegates rather than
  duplicates.
- **`PHASE_6_PLAN.md`** — Day 5 banner update only; the day's body is
  unchanged because Day 5a is a strict subset of the day's original
  prompt (the operator-side 4 gates are deferred to Day 5b).

---

## 3. Quality gates — Day 5a (5 of 5 green)

Day 5a's gates are a strict subset of the original Day-5 prompt's 8
gates — the 4 operator-side gates (1, 4, 5 in the original numbering
+ the first probe success) are explicitly out-of-scope today; the
agent-deliverable gates are all in.

### Gate 5a-1 — `scripts/ci/setup-mac-runner.sh` syntax + clean exit

```text
$ bash -n scripts/ci/setup-mac-runner.sh && echo "syntax OK"
syntax OK

$ bash scripts/ci/setup-mac-runner.sh >/dev/null 2>&1; echo "exit=$?"
exit=0
```

✅ Recipe parses + runs end-to-end with `exit=0` on the dev Mac.

### Gate 5a-2 — Live preflight diagnostics readable + correct

Captured in § 2.1 above. ✅ Each numbered section emits clear pass/fail
lines; the Day-4b artefact probe correctly identifies the missing
vmlinux + uncached Class-E helpers + does not exit non-zero (the
smokes do that gating themselves at run time).

### Gate 5a-3 — `components-json-verify.sh` still green (no Day-5 regression)

```text
[components-json-verify] OK
  Class A (host binaries):    7/7 green
  Class B (microVM bundles):  1/1 green (D.2.a share-bundle invariant enforced)
  Class C (kernel):           1/1 green (structural; checksum populated by Day-4b operator handoff)
  Class D (linux-only):       1/1 green (darwin absent as required)
  Class E (3rd-party):        3/3 green (real url + checksum)
  Capsules projection:        10 entries include 'aarch64-darwin'
```

✅ Day-4a's invariant state preserved.

### Gate 5a-4 — `cross-platform-test.sh` 47/47

```text
cross-platform.sh: 47 passed, 0 failed
```

✅ Phase-5-Day-8 baseline preserved.

### Gate 5a-5 — Mac pre-flight still 3/3 (no Day-3 regression)

```text
PASS: local-carrier-setup-smoke.sh
PASS: home-frontdoor-smoke.sh
PASS: chat-wasm-native-interop-smoke.sh
```

✅ Day-3's headline outcome preserved.

### Gate 5a-6 (extra) — Workflow YAML still parses

```text
mac-vz.yml YAML OK
_self-hosted-probe.yml YAML OK
```

✅ Day-5a did not break the Phase-5-Day-6 workflow.

### Gate 5a-7 (extra) — Diff scope

3 files touched + 1 new (this notes file pending):

```text
M  docs/vz-backend/CI_RUNBOOK.md
M  docs/vz-backend/SELF_HOSTED_RUNNER_SPEC.md
?? scripts/ci/setup-mac-runner.sh
```

✅ Plus this notes file + Day-5 banner edit in `PHASE_6_PLAN.md`. Total
5 files in the Day-5a commit. Comfortably under the 7-file Day-4a
budget — Day 5a is the smaller-day-of-the-block by design.

---

## 4. The 4 deferred operator gates (Day 5b)

The original Day-5 prompt's gates 1, 3, 4, 5 are operator-side and need
real hardware + GitHub registration token + repo-admin credentials.
Tracked here for the operator handoff:

### Gate 5b-1 — Self-hosted runner provisioned

**Operator action:** procure an Apple-Silicon Mac matching the floors
in spec § 2 (≥ 16 GiB RAM, ≥ 100 GiB free disk, macOS ≥ 13). Boot it,
log in as the operator account.

**Success signal:** physical machine reachable; the operator can `ssh`
or sit at it.

### Gate 5b-2 — Setup recipe green on the physical Mac

**Operator action:**

```sh
cd /path/to/elastos-runtime
bash scripts/ci/setup-mac-runner.sh
```

**Success signal:** `exit=0`; final block prints "Provisioning preflight
GREEN. Continue with Day-5b operator handoff." (the exact same green
this Day-5a commit's dev-Mac run produced).

If any check fails, the typed exit code names the failure class; the
recipe's diagnostics name the exact remediation.

### Gate 5b-3 — Runner agent registered + repo var set

**Operator action:** follow spec § 4.3 (runner registration, labels
`self-hosted,macOS,ARM64,vz-capable`, `./svc.sh install + ./svc.sh
start`); then:

```sh
gh variable set MAC_VZ_FULL_BOOT_ENABLED --repo <owner>/<repo> --body true
```

**Success signal:** the runner shows **Idle** in the GitHub Actions UI;
the variable shows in the Settings → Actions → Variables list.

### Gate 5b-4 — `_self-hosted-probe.yml::probe-attempt` completes

**Operator action:** Actions UI → Self-hosted Mac runner probe → Run
workflow.

**Success signal:** the `probe-attempt` job completes in < 1 min with
`Virtualization.framework PRESENT` printed. The `probe-fallback` job
on `ubuntu-latest` prints `MAC_VZ_FULL_BOOT_ENABLED=true`.

**Expected wall-clock:** < 5 min from "Run workflow" click to green.

### Total Day-5b operator wall-clock estimate

| Step | Wall-clock |
|---|---|
| Procure HW (if not already on hand) | 0..N days (depends on operator) |
| Boot + `bash setup-mac-runner.sh` | ~5 min |
| Runner agent install + label registration | ~10 min |
| `gh variable set` | ~30 s |
| `_self-hosted-probe.yml::probe-attempt` run | ~3 min |
| **Total active operator time** | **~20 min** (excluding HW procurement + Day-4b artefact build) |

Day 5b is *not* on the Day-6 critical path: Day 6 needs the lane
active to run the first `mac-vz-full-boot` end-to-end, so Day 5b
gates Day 6. But within Day 5b the 20-minute operator window can be
done in any sitting.

### Day-4b + Day-5b ordering note

**Day-4b can happen before OR after Day-5b.** Day-4b produces the
`vmlinux` Image; Day-5b stands the runner up. If the operator runs
Day-5b first, the `mac-vz-full-boot` job will exit with a typed
"vmlinux not found" error (still a real test signal — proves the
runner is alive and the substrate is wired); after Day-4b lands, the
job goes fully green. The recipe in § 2.1 handles either ordering
correctly via the informational-only Day-4b artefact probe.

---

## 5. Carry-forward to Day 6 + Phase 7

### Day-6 unblocked

The `mac-vz-full-boot` workflow job + `_self-hosted-probe.yml` exist
from Phase 5 Day 6; the setup recipe + spec/runbook updates are the
Day-5a contribution. With those, the operator can complete Day 5b in
any 20-minute sitting (excluding HW procurement). **Day 6 is unblocked
modulo Day 5b's operator wall-clock.**

### Phase-7 carry-forward (cross-referenced with prior days)

- **`just provision-mac-runner` shim.** Day 5a chose not to add the
  `just` wrapper to avoid a chicken-and-egg `just`-on-runner-first
  dependency. If `just` becomes a runner prereq for other reasons in
  Phase 7, fold the recipe in.
- **Multi-runner fleet.** Day 5a designs for one runner; the spec § 7
  notes a fleet is Phase-7 work.
- **Auto-recovery / agent monitoring.** If the runner kernel-panics,
  an operator has to reboot it. No out-of-band recovery channel today.

---

## 6. Day-6 entry signal

Day 6 may start when:

- [x] All 5 Day-5a quality gates green (§ 3).
- [x] Spec + runbook updates landed + reference the new recipe.
- [x] Setup recipe live-tested on the dev Mac (clean `exit=0`).
- [x] No regression on Day-3's 3/3 Mac pre-flight outcome.
- [ ] Day-5b operator handoff complete: runner registered, variable
      set, `_self-hosted-probe.yml::probe-attempt` green at least once.

Four of five signals met at the time of this commit. The fifth is the
Day-5b operator handoff; Day 6 can begin its agent-side planning work
in parallel with Day 5b, but the Day-6 first-FORCE_FULL-smoke headline
gate (Gate 6-1) requires Day 5b complete first.

---

**End of PHASE_6_DAY_5_NOTES.md.**
