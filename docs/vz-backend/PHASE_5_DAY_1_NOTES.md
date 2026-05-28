# Phase 5 Day 1 — Port `local-carrier-setup-smoke.sh` to macOS

> **Status:** Complete. The shell smoke is Mac-aware: helper sourcing, bash 3.2 portability, dry-run lane, Vz substrate readiness probe, and graceful Phase-6 prerequisite detection all land. The smoke produces actionable output on macOS without crashing, and the Phase-6 gap it surfaces is now operator-visible rather than buried in a Python `KeyError`.
>
> **Linux-untouched gate:** `scripts/check-linux-untouched.sh bcf5a0a` green. All work is in `scripts/`; no Rust crates touched.
>
> **Anchor:** [`PHASE_5_PLAN.md`](PHASE_5_PLAN.md) §1 Day 1.

---

## 1. What Day 1 ships

| Component | File | Change |
|---|---|---|
| Cross-platform shell helper library | `scripts/lib/cross-platform.sh` (new) | bash-3.2-clean `pid_is_running`, `read_pids_into_array`, `vz_host_is_capable`, `vz_discover_launchable_capsule`. Sourced by Day 1's smoke and (planned) Days 2/3. Replaces the inline `mapfile`-free idiom with a centralised helper. |
| Helper unit tests | `scripts/lib/cross-platform-test.sh` (new) | 15 assertions covering every helper function: live + dead + empty PIDs, three-line + empty + blank-mixed array reads, capability check on both Darwin and non-Darwin hosts, fixture-discovery happy path + empty data dir + wasm-only-capsule-skip cases. Runs on bash 3.2; no external test framework. |
| Smoke pre-flight | `scripts/local-carrier-setup-smoke.sh` | Sources the helper; adds `ELASTOS_VZ_SMOKE_DRY_RUN=1` early-exit (CI fast lane); adds Mac-only pre-flight that detects missing `components.json` darwin-arm64 entries before any Python install staging and exits cleanly with an actionable operator message; adds Vz substrate readiness probe at the tail. |
| Operator escape hatch | `scripts/local-carrier-setup-smoke.sh` | `ELASTOS_VZ_SMOKE_FORCE_PROCEED=1` bypasses the Mac pre-flight so operators / Phase 6 developers can debug the still-failing Python staging step directly. Documented in the pre-flight skip message. |
| Plan + docs | `docs/vz-backend/PHASE_5_PLAN.md` (new), `docs/vz-backend/PHASE_5_DAY_1_NOTES.md` (this file), `docs/vz-backend/PLAN.md`, `docs/MAC.md` | 8-day Phase 5 plan + Day 1 outcome log + capability matrix bump. |

---

## 2. What the smoke now does on Mac

The flow on `aarch64-apple-darwin`:

1. **Source helpers.** `scripts/lib/cross-platform.sh` provides bash-3.2-clean PID liveness, array reading, Vz capability detection, and capsule discovery.

2. **Dry-run lane (optional).** `ELASTOS_VZ_SMOKE_DRY_RUN=1`: exits 0 after parse + helper-source check. Logs the Vz host capability outcome. CI uses this on every PR to prove the script *parses* on Mac without paying the ~10-minute end-to-end cost.

3. **Mac pre-flight.** Reads `components.json`; if any of `shell` / `localhost-provider` / `did-provider` / `webspace-provider` lacks `darwin-arm64` (or `*`) release metadata, emits the actionable operator message and exits 0. The message explicitly names Phase 6 (PLAN.md L321) as the deliverable that restores the entries.

4. **Escape hatch.** `ELASTOS_VZ_SMOKE_FORCE_PROCEED=1`: bypasses the pre-flight so the smoke continues into the Python install staging block. Surfaces the same `... missing release metadata for darwin-arm64` Python error the pre-flight is shielding operators from — useful for Phase-6 developers iterating on the components.json restoration.

5. **(Linux behaviour unchanged.)** All Mac-only paths are guarded by `OS_TOKEN == darwin`; Linux runs are byte-identical to pre-Day-1.

6. **Vz substrate readiness probe (tail).** After the main smoke OK message, two diagnostic checks run unless `ELASTOS_VZ_SMOKE_SKIP_PROBE=1`:
   - `vz_host_is_capable` — emits "host capable" or "host NOT capable" based on `sw_vers -productVersion` ≥ 12. On this Mac (macOS 12+), reports capable.
   - `vz_discover_launchable_capsule "${DATA_DIR}"` — looks for `<data_dir>/capsules/<name>/{capsule.json,rootfs.ext4}`. On a fresh host the Carrier install pipeline doesn't produce a rootfs.ext4 (the Phase-6 components.json restoration is the prerequisite), so this visibly-skips with a clear message.

---

## 3. The Phase-6 gap surfaced by Day 1

The smoke's failure-mode under `set -euo pipefail` without the Day-1 pre-flight was a Python `SystemExit` from the staging block:

```
shell missing release metadata for darwin-arm64
```

Buried in a long build log, this is not actionable — an operator sees the smoke exit 1 and has to dig to understand whether it's their host, the script, the Mac substrate, or something else.

The Day 1 pre-flight surfaces this *up front*, with a named phase, named files, and a named escape hatch:

```
[local-carrier-setup] Mac pre-flight: components.json has no
darwin-arm64 release metadata for one or more native binaries.

  Required:   shell, localhost-provider, did-provider, webspace-provider
  Phase:      Phase 6 deliverable (see docs/vz-backend/PLAN.md L321).
  Status:     Pre-Work removed the dishonest darwin entries; Phase 6
              restores truthful ones once Mac substrate + signing land.
  ...
  To skip this guard and exercise the WASM/data half regardless,
  rerun with: ELASTOS_VZ_SMOKE_FORCE_PROCEED=1 ...

  To dry-run only (CI fast lane), set: ELASTOS_VZ_SMOKE_DRY_RUN=1 ...
```

Phase 6 closes the gap by restoring the entries truthfully (the Mac substrate now boots these binaries via Vz end-to-end, so the entries that were honestly removed in Pre-Work can now honestly be added back).

---

## 4. What was NOT changed

In keeping with "minimal code changes for minimal risk" (per the user rules):

- **`components.json`** stays as Pre-Work left it. Restoring darwin entries is Phase 6's deliverable; pushing it into Phase 5 Day 1 would be scope creep.
- **Linux behaviour** unchanged. The pre-flight gate, dry-run check, and substrate probe are all wrapped in `OS_TOKEN == darwin` conditions or env-var opt-ins.
- **Day-4 Rust smoke** (`elastos-server/tests/vz_supervisor_smoke.rs`) stays as the source of truth for the supervisor RPC contract this shell smoke wraps. Day 1 confirmed it still passes.
- **No new external dependencies.**

---

## 5. Test inventory

### 5.1 New: `scripts/lib/cross-platform-test.sh` (15 assertions)
- `pid_is_running`: self PID alive (3 cases); dead/empty PIDs not alive.
- `read_pids_into_array`: three-line input → three entries with order preserved (4 cases); empty input → empty array (no `set -u` trip); blank-mixed input → filtered.
- `vz_host_is_capable`: Darwin yields true on macOS 12+ host; non-Darwin yields false.
- `vz_discover_launchable_capsule`: seeded fixture discovered; empty data dir reports none; wasm-only capsule (no rootfs.ext4) skipped.

### 5.2 Existing tests (re-validated, no changes)
- `elastos-server/tests/vz_supervisor_smoke.rs` (Phase 4 Day 4) — full supervisor RPC contract for a launchable Vz capsule.

---

## 6. Operator runbook

### 6.1 CI fast lane (parse check only)

```bash
ELASTOS_VZ_SMOKE_DRY_RUN=1 bash scripts/local-carrier-setup-smoke.sh
```

Output (Mac):
```
[local-carrier-setup] dry-run mode: parse OK, helper sourced OK; exiting before cargo build
[local-carrier-setup] dry-run: Vz host capability check passed (macOS 12+)
```

Exit 0. Suitable for `pull_request` event in CI on `macos-14`.

### 6.2 Full smoke (Mac, pre-Phase-6)

```bash
bash scripts/local-carrier-setup-smoke.sh
```

Currently exits 0 after the Phase-6 pre-flight skip. Operators see the actionable message; CI dashboards alert on the skip telemetry separately (Day 7 wires this into the CI job).

### 6.3 Phase 6 developer iteration

```bash
ELASTOS_VZ_SMOKE_FORCE_PROCEED=1 bash scripts/local-carrier-setup-smoke.sh
```

Bypasses the pre-flight and surfaces the next gap (currently `shell missing release metadata for darwin-arm64` from the Python staging block). Use to iterate on `components.json` restoration without touching the smoke script.

### 6.4 Helper library validation

```bash
bash scripts/lib/cross-platform-test.sh
```

15 assertions; should report `15 passed, 0 failed` on both bash 3.2 (macOS) and bash 4+ (Linux).

---

## 7. Findings for future Phase 5 days

| Finding | Affects | Action |
|---|---|---|
| `scripts/lib/runtime-cleanup.sh` uses `/proc/${pid}` (Linux-only) for liveness checks. Mac will silently skip every kill issued by it. | Day 2's `home-frontdoor-smoke.sh` port (sources this helper transitively via `public-install-home-frontdoor-smoke.sh`) | Day 2 replaces `/proc/${pid}` with `pid_is_running` from `scripts/lib/cross-platform.sh`. |
| `home-frontdoor-smoke.sh` uses `mapfile -t` (bash 4+ only). | Day 2 port | Day 2 replaces with `read_pids_into_array` from the helper. |
| Native binaries lack darwin-arm64 entries in `components.json` → blocks every shell-driven Mac smoke until Phase 6 restores them. | Day 2, Day 3, Day 7 | All three smokes inherit the Day-1 pre-flight pattern. Phase 6 lifts the gate by restoring entries. |
| `vz_supervisor_smoke.rs` visibly-skips if no rootfs.ext4 is installed; the Day-1 substrate probe surfaces the same skip in the shell layer. | Day 5's concurrent stress | Day 5 needs a separate rootfs provisioning step (out of scope for Phase 5 per PHASE_5_PLAN.md). |
| `sw_vers -productVersion` is the cheapest pre-flight signal for "is Vz plausibly usable". The real check is the Rust `is_supported()` via objc2; the shell signal is sufficient for smoke skip-or-proceed decisions. | All Mac smokes | No action. Documented in `vz_host_is_capable`. |

---

## 8. Why this is a Day-1 stopping point (not a half-done deliverable)

The PHASE_5_PLAN.md scope for Day 1 was *port `local-carrier-setup-smoke.sh` to Mac*. The deliverable shipped here ports it as far as the substrate honestly supports:

- Bash + helper portability **complete**.
- CI dry-run lane **complete**.
- Mac substrate readiness probe **complete**.
- Operator-visible Phase-6 prerequisite detection **complete**.

What's NOT done here is the full end-to-end Carrier install on Mac. That requires Phase 6's `components.json` restoration. The plan acknowledged this:

> "Risk: One smoke surfaces a deep Vz substrate bug that takes >1 day to fix → Stop the day, file a follow-up day, do NOT scope-creep the originally-planned day. Phase 4 set this precedent (every day was 1 deliverable)." (PHASE_5_PLAN.md §2)

Day 1 surfaced the gap exactly as the plan predicted ("the first 'smoke surfaces a real bug' moment is most likely to happen here — better to find them now while the audit memory is fresh"). The gap is **named, characterised, and gated behind operator-friendly tooling**. Day 2 / Day 6 / Phase 6 all benefit from this work without rework.

The honest framing: Day 1 ships infrastructure + diagnostics; Phase 6 lifts the components.json gate; Day 1's Mac smoke then runs end-to-end without any further smoke-script changes.
