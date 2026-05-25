#!/usr/bin/env bash
# Phase 5 Day 7 — Vz performance-baseline measurement script.
#
# Runs the `vz_perf_harness` integration test 5 times against
# the synthetic code paths the Phase-4/5 substrate exposes,
# aggregates the per-metric JSONL into a canonical baseline
# JSON object, and writes it to `target/vz-baseline.json`.
#
# Real Vz boot timings are gated on Phase 6 (`components.json`
# darwin-arm64 release metadata not yet published); the
# harness's JSON `notes.real_vz_boot_measured: false` plus
# the visible summary at the tail surface that fact loud and
# clear. See `docs/vz-backend/PERFORMANCE_BASELINE.md`.
#
# Operational lanes (Day 5/6 precedence — top wins):
#   1. ELASTOS_VZ_SMOKE_FORCE_FULL=1 → full run, even in CI.
#   2. ELASTOS_VZ_SMOKE_DRY_RUN=0    → full run.
#   3. ELASTOS_VZ_SMOKE_DRY_RUN=1    → dry run.
#   4. CI auto-detect (Day 5)        → dry run.
#   5. Default                       → full run.
#
# Bash 3.2 clean, BSD-utils compatible (macOS default).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# shellcheck source=lib/cross-platform.sh
. "${REPO_ROOT}/scripts/lib/cross-platform.sh"

# OS gate. The script runs on macOS (canonical Vz lane) plus
# a Linux self-test path (the harness compiles + runs on
# Linux against the same synthetic paths). Other OSes exit.
case "$(uname -s)" in
    Darwin) BACKEND_LABEL="vz" ;;
    Linux)  BACKEND_LABEL="vz-on-linux-self-test"
            echo "[measure-vz-baseline] note: running on Linux. The harness exercises the SAME synthetic code paths the Mac/Vz lane does, but the canonical Vz lane is Mac-only. Use measure-crosvm-baseline.sh for the crosvm comparison."
            ;;
    *)
        echo "[measure-vz-baseline] unsupported OS: $(uname -s)" >&2
        exit 1
        ;;
esac

# Phase 5 Day 6 — FORCE_FULL override (highest precedence).
if [[ "${ELASTOS_VZ_SMOKE_FORCE_FULL:-0}" == "1" ]]; then
    echo "[measure-vz-baseline] FORCE_FULL=1 — forcing full measurement run (overrides CI auto-detect)"
    export ELASTOS_VZ_SMOKE_DRY_RUN=0
fi

# Phase 5 Day 5 — auto-dry-run in CI.
if [[ -z "${ELASTOS_VZ_SMOKE_DRY_RUN:-}" ]] && cross_platform_in_ci; then
    echo "[measure-vz-baseline] CI detected (GITHUB_ACTIONS or CI env set); auto-enabling ELASTOS_VZ_SMOKE_DRY_RUN=1"
    export ELASTOS_VZ_SMOKE_DRY_RUN=1
fi

# Dry-run lane: parse + helper-source check only. CI smoke for
# the script itself; doesn't pay the harness's wall-clock cost.
if [[ "${ELASTOS_VZ_SMOKE_DRY_RUN:-0}" == "1" ]]; then
    echo "[measure-vz-baseline] dry-run mode: parse OK, helper sourced OK; exiting before cargo test"
    if [[ "$(uname -s)" == "Darwin" ]]; then
        if vz_host_is_capable; then
            echo "[measure-vz-baseline] dry-run: Vz host capability check passed (macOS 12+)"
        else
            echo "[measure-vz-baseline] dry-run: Vz host capability check FAILED (host lacks Vz)"
            exit 1
        fi
    fi
    exit 0
fi

# Full measurement run.
ELASTOS_ROOT="${REPO_ROOT}/elastos"
TARGET_DIR="${ELASTOS_ROOT}/target"
REPORT_DIR="${TARGET_DIR}"
mkdir -p "${REPORT_DIR}"
JSONL_PATH="${REPORT_DIR}/vz-baseline.jsonl"
BASELINE_PATH="${REPORT_DIR}/vz-baseline.json"

RUNS="${ELASTOS_VZ_PERF_RUNS:-5}"
echo "[measure-vz-baseline] starting ${RUNS} runs (backend=${BACKEND_LABEL})"
echo "[measure-vz-baseline] JSONL → ${JSONL_PATH}"
echo "[measure-vz-baseline] aggregated → ${BASELINE_PATH}"

# Fresh JSONL per invocation so reruns don't accumulate stale
# samples. The aggregated baseline at the tail is the canonical
# output; the JSONL is the audit trail.
: > "${JSONL_PATH}"

run_idx=1
while [[ "${run_idx}" -le "${RUNS}" ]]; do
    echo "[measure-vz-baseline] run ${run_idx}/${RUNS}…"
    ELASTOS_VZ_PERF_REPORT="${JSONL_PATH}" cargo test \
        --manifest-path "${ELASTOS_ROOT}/Cargo.toml" \
        -p elastos-server \
        --test vz_perf_harness \
        -- --test-threads=1 \
        >/dev/null 2>&1 || {
            echo "[measure-vz-baseline] run ${run_idx} FAILED — aborting" >&2
            exit 1
        }
    run_idx=$((run_idx + 1))
done

# Aggregate the JSONL into a single baseline JSON. Pure
# `python3` so we don't need `jq` (not installed on every dev
# Mac). The aggregator emits the MEDIAN of each metric's per-
# run stats — the "median run" semantics from the Day-7
# prompt. See PERFORMANCE_BASELINE.md § Methodology.
python3 - "${JSONL_PATH}" "${BASELINE_PATH}" "${BACKEND_LABEL}" <<'PY'
import json
import statistics
import sys
import time

jsonl_path, baseline_path, backend_label = sys.argv[1], sys.argv[2], sys.argv[3]

per_metric = {}
host = None
notes = None
schema_version = None
with open(jsonl_path) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        record = json.loads(line)
        per_metric.setdefault(record["metric_name"], []).append(record["stats"])
        # Last-writer-wins for host/notes/schema — they're stable across runs.
        host = record["host"]
        notes = record["notes"]
        schema_version = record["schema_version"]

def median_of(samples_for_key, key):
    values = [s[key] for s in samples_for_key]
    if not values:
        return 0
    return int(statistics.median(values))

metrics = {}
for metric, samples in sorted(per_metric.items()):
    metrics[metric] = {
        "runs": len(samples),
        "samples_per_run": samples[0]["samples_count"] if samples else 0,
        "median_min_us":     median_of(samples, "min_us"),
        "median_p50_us":     median_of(samples, "p50_us"),
        "median_p95_us":     median_of(samples, "p95_us"),
        "median_p99_us":     median_of(samples, "p99_us"),
        "median_max_us":     median_of(samples, "max_us"),
    }

baseline = {
    "schema_version": schema_version or 1,
    "captured_at_unix_ms": int(time.time() * 1000),
    "host": host or {},
    "backend": backend_label,
    "notes": notes or {
        "real_vz_boot_measured": False,
        "real_vz_boot_blocker": "Phase 6 — components.json missing darwin-arm64 release metadata",
    },
    "metrics": metrics,
}

with open(baseline_path, "w") as f:
    json.dump(baseline, f, indent=2, sort_keys=True)
    f.write("\n")

# Human-readable summary.
print()
print(f"=== Vz baseline ({backend_label}) ===")
print(f"  host:   {host.get('os','?')}/{host.get('arch','?')}  cpu_count_logical={host.get('cpu_count_logical','?')}")
print(f"  phase:  {host.get('phase','?')}  runs={len(next(iter(per_metric.values()), []))}")
print()
print(f"  {'metric':<45} {'samples':>8} {'p50':>10} {'p95':>10} {'p99':>10} {'max':>10}")
print(f"  {'-' * 45} {'-' * 8} {'-' * 10} {'-' * 10} {'-' * 10} {'-' * 10}")
for metric, agg in metrics.items():
    print(
        f"  {metric:<45} {agg['samples_per_run']:>8} "
        f"{agg['median_p50_us']:>8} µs {agg['median_p95_us']:>8} µs "
        f"{agg['median_p99_us']:>8} µs {agg['median_max_us']:>8} µs"
    )
print()
if not baseline['notes'].get('real_vz_boot_measured', False):
    print(f"  NOTE: real Vz boot timings NOT measured.")
    print(f"        Blocker: {baseline['notes'].get('real_vz_boot_blocker','?')}")
print()
PY

echo "[measure-vz-baseline] OK — baseline written to ${BASELINE_PATH}"
