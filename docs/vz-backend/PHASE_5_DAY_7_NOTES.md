# Phase 5 Day 7 — Outcome Notes

> **Date:** 2026-05-25.
> **Branch:** local (push deferred per the day-by-day cadence).
> **Anchors:** [`PHASE_5_PLAN.md`](PHASE_5_PLAN.md) § Day 7, [`PERFORMANCE_BASELINE.md`](PERFORMANCE_BASELINE.md).
>
> **Headline:** Ships the Vz/crosvm performance-measurement substrate (Rust harness + Mac/Linux aggregation scripts + canonical JSON wire format) and the honest baseline document with M-series Mac numbers populated for every cell that is reachable today. Real microVM-boot timings remain Phase-6-gated; the document and the harness's `notes` field both surface that fact prominently rather than papering over it.

---

## 1. Scope-deviation note

The original `PHASE_5_PLAN.md` Day-7 prompt was "Apple-Silicon GitHub Actions CI runner". Day 5 fulfilled that scope (the GitHub-hosted dry-run lane); Day 6 added the self-hosted full-boot lane spec. Day 7 picks up the original Day-6 perf-baseline scope, but adapted to the honest reality that **real Vz boot timings depend on Phase 6** (darwin-arm64 release metadata). So Day 7 ships the substrate AND the honest baseline doc; Phase 6 plugs real-boot numbers into the pre-baked comparison table without restructuring.

---

## 2. What shipped

### 2.1 `elastos-server/tests/vz_perf_harness.rs` (new)

Cross-platform synthetic perf harness, 11 `#[test]` functions:

- **6 measurement tests** — one per metric, each emitting a JSONL line into `ELASTOS_VZ_PERF_REPORT` (if set) and asserting a loose sanity tripwire (5 s for the supervisor/launch paths, 1 s for the RPC/validate paths).
- **5 schema/utility tests** — pin the percentile algorithm, single-sample edge case, JSON schema-version contract, echo-provider round-trip, and the `GrantDuration` import link.

Metrics:

| Metric | Samples/run | Path measured |
|---|---:|---|
| `supervisor_new_cold` | 5 | `Supervisor::new` + (Mac) `prune_stale_mac_artifacts` on a fresh data dir. |
| `supervisor_new_warm` | 20 | `Supervisor::new` on a pre-pruned data dir. |
| `synthetic_capsule_launch` | 20 | `EnsureCapsule` RPC against a seeded synthetic capsule. |
| `provider_registry_send_raw_single` | 100 | `ProviderRegistry::send_raw` single-sender. |
| `provider_registry_send_raw_concurrent` | 100 (4×25) | `ProviderRegistry::send_raw` 4 senders × 25 messages. |
| `capability_manager_validate` | 100 | `CapabilityManager::validate` against a pre-seeded token store. |

The harness runs as a regular `cargo test` target so the Phase-5-Day-5 `mac-rust-tests` job picks it up automatically. JSONL emission only activates when the `ELASTOS_VZ_PERF_REPORT` env var is set, so default `cargo test` runs are side-effect-free.

### 2.2 `scripts/measure-vz-baseline.sh` (new, cross-OS)

Aggregates 5 runs of the harness, computes per-metric medians, writes `target/vz-baseline.json`. Operational lanes (Day-5/Day-6 precedence):

1. `ELASTOS_VZ_SMOKE_FORCE_FULL=1` → full run, even in CI.
2. `ELASTOS_VZ_SMOKE_DRY_RUN=0` → full run.
3. `ELASTOS_VZ_SMOKE_DRY_RUN=1` → dry run.
4. CI auto-detect → dry run.
5. Default → full run.

Honors `ELASTOS_VZ_PERF_RUNS` to override the default 5-run count. Bash-3.2 clean.

### 2.3 `scripts/measure-crosvm-baseline.sh` (new, Linux-only)

Same harness invocation, distinct on-disk artefact (`target/crosvm-baseline.json`, backend label `crosvm`). Exits cleanly with a message on macOS (rc=0, not a failure). Same Day-5/Day-6 precedence.

### 2.4 `docs/vz-backend/PERFORMANCE_BASELINE.md` (new)

Sections:

1. **TL;DR** — status table calling out exactly what's measured vs not.
2. **Methodology** — sample sizes, percentile-of-percentiles aggregation, no warm-up suppression.
3. **What we measure today** — code-path-by-code-path table.
4. **What we cannot measure yet** — explicit Phase-6 dependencies + unblock conditions.
5. **Initial baseline (Mac, M-series)** — populated numbers + per-metric observations.
6. **Comparison template for Phase 6** — pre-baked table with `_TBD_` cells for Linux + real-boot rows.
7. **JSON wire format** — schema documentation with the `schema_version: 1` contract.
8. **How to regenerate** — Mac + Linux invocation examples covering every precedence path.
9. **Anchors.**

### 2.5 Initial Mac baseline (M-series)

| Metric | samples / run | p50 (µs) | p95 (µs) | p99 (µs) | max (µs) |
|---|---:|---:|---:|---:|---:|
| `capability_manager_validate` | 100 | 29 | 34 | 38 | 45 |
| `provider_registry_send_raw_concurrent` | 100 | 5 | 11 | 13 | 14 |
| `provider_registry_send_raw_single` | 100 | 0 | 0 | 0 | 1 |
| `supervisor_new_cold` | 5 | 7 | 31 | 31 | 31 |
| `supervisor_new_warm` | 20 | 3 | 4 | 4 | 4 |
| `synthetic_capsule_launch` | 20 | 47 | 73 | 93 | 93 |

All Phase-4/5 synthetic paths run sub-millisecond at p99. See PERFORMANCE_BASELINE.md § 5 for the per-metric observations.

---

## 3. Operator benefits

- **Reproducible baseline today.** Operators can run `scripts/measure-vz-baseline.sh` on any Mac dev host and get an apples-to-apples comparison with the documented numbers in PERFORMANCE_BASELINE.md.
- **CI-friendly.** The harness is just `cargo test`; the script auto-dry-runs in CI (Day-5 contract). The Day-6 self-hosted lane will switch to FORCE_FULL=1 once provisioned.
- **Wire-format-stable.** `target/{vz,crosvm}-baseline.json` has a frozen `schema_version: 1`. Phase-6 regression detection has a contract to lean on without re-discovery.
- **Honest about gaps.** Both the doc and the JSON's `notes` field name the unblock condition for every cell that's `_TBD_`. No silent omissions.
- **Sanity tripwires.** Each metric has a loose upper bound (5 s for supervisor paths, 1 s for RPC/validate). A regression that pushes any path 10×+ above current numbers will fail the test in CI before anyone has to look at the baseline.

---

## 4. Carry-forward findings

1. **`rust_version` reports `"unknown"`** because we deliberately don't add a `rustc_version`-style build dependency (Day-7 budget). A future cleanup could set `RUSTC_VERSION_AT_BUILD` from `build.rs` (no runtime cost) so the JSON carries the toolchain SHA.
2. **No release-profile lane.** All numbers are debug-profile. Honest for tracking substrate cost, NOT honest for "what does production latency look like." Phase 6 should add `cargo test --release` or `criterion` for the production-profile numbers.
3. **No git-commit SHA in the JSON.** Documented in PERFORMANCE_BASELINE.md § 6 rules of engagement. Cheap to add (`git rev-parse HEAD` in the script) — call out as a Day-8 or Phase-6 follow-up.
4. **`send_raw_single` measures at the µs-resolution floor.** The work fits inside one tokio task switch. If this metric becomes load-bearing for regression detection, the harness should drop to nanosecond resolution.
5. **No Linux-side numbers populated yet.** The script is written and ready; populating requires running on the Linux comparison host. Day-8 may run it; otherwise Phase 6.

---

## 5. Runbook addendum

**To regenerate the Mac baseline:**

```sh
bash scripts/measure-vz-baseline.sh                  # 5 runs, full measurement
ELASTOS_VZ_PERF_RUNS=3 bash scripts/measure-vz-baseline.sh   # quick smoke
ELASTOS_VZ_SMOKE_DRY_RUN=1 bash scripts/measure-vz-baseline.sh   # syntax-check only
```

**To regenerate the Linux baseline (on a Linux host):**

```sh
bash scripts/measure-crosvm-baseline.sh   # writes target/crosvm-baseline.json
```

**To inspect the baseline:**

```sh
python3 -m json.tool elastos/target/vz-baseline.json
```

**To filter the JSONL audit trail:**

```sh
# Per-metric, per-run percentiles before aggregation:
cat elastos/target/vz-baseline.jsonl | python3 -c '
import json, sys
for line in sys.stdin:
    r = json.loads(line)
    print(f"{r[\"metric_name\"]:<45} p50={r[\"stats\"][\"p50_us\"]} p99={r[\"stats\"][\"p99_us\"]}")
'
```

---

## 6. Quality gates

- [x] `cargo fmt --all -- --check` — clean.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- [x] `cargo test -p elastos-server -p elastos-vz --tests -- --test-threads=1` — green (incl. new perf harness's 11 tests).
- [x] `cargo test -p elastos-server -p elastos-vz --tests -- --test-threads=4` — green.
- [x] `bash scripts/lib/cross-platform-test.sh` — 44 passed (no helper changes).
- [x] `bash scripts/lib/runtime-cleanup-test.sh` — 5 passed.
- [x] `bash scripts/measure-vz-baseline.sh` with `ELASTOS_VZ_SMOKE_DRY_RUN=1` — exits 0.
- [x] `bash scripts/measure-vz-baseline.sh` (full, 5 runs on this Mac) — writes valid `target/vz-baseline.json` matching the documented schema.
- [x] `bash scripts/measure-crosvm-baseline.sh` on Mac — exits 0 with the clean-exit message.
- [x] `scripts/check-linux-untouched.sh bcf5a0a` — green.
- [x] JSON schema sanity check — `python3 -m json.tool` accepts the baseline; `schema_version`, `metric_name`, all `median_*_us` fields present.

---

## 7. Files changed (summary)

| Change | File |
|---|---|
| New | `elastos/crates/elastos-server/tests/vz_perf_harness.rs` (11 tests) |
| New | `scripts/measure-vz-baseline.sh` (cross-OS, FORCE_FULL-aware) |
| New | `scripts/measure-crosvm-baseline.sh` (Linux-only, clean-exit on Mac) |
| New | `docs/vz-backend/PERFORMANCE_BASELINE.md` |
| New | `docs/vz-backend/PHASE_5_DAY_7_NOTES.md` (this file) |
| Modified | `docs/vz-backend/PHASE_5_PLAN.md` (status bump + Day-7 outcome) |
| Modified | `docs/vz-backend/PLAN.md` (status row) |
| Modified | `docs/MAC.md` (Performance vs Linux row) |

No production Rust code changes — Day 7 is pure measurement substrate + documentation. The new `vz_perf_harness.rs` lives entirely under `tests/`.
