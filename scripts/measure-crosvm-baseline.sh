#!/usr/bin/env bash
# Phase 5 Day 7 — crosvm performance-baseline measurement script.
#
# Companion to `scripts/measure-vz-baseline.sh`. Runs the
# SAME `vz_perf_harness` integration test 5 times against the
# synthetic code paths, but emits its output as
# `target/crosvm-baseline.json` (backend label "crosvm") so
# the Mac/Linux delta in `docs/vz-backend/PERFORMANCE_BASELINE.md`
# is apples-to-apples — same source, same Rust crate, same
# tokio scheduler, only the host OS + (eventual) microVM
# substrate differs.
#
# Real microVM boot timings are out of scope for Day 7. The
# harness exercises Rust-level paths only (orphan-cleanup,
# `ProviderRegistry::send_raw`, `CapabilityManager::validate`,
# synthetic capsule launch). Phase-6 follow-up adds the real
# microVM boot path on both substrates.
#
# Behaviour matrix:
#   - On Linux: runs the harness, writes the canonical JSON.
#   - On macOS: exits cleanly with a clear message — use the
#     Vz-side script instead.
#   - On other OS: errors out.
#
# Honors the same Day-5/Day-6 precedence as the Vz script.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# shellcheck source=lib/cross-platform.sh
. "${REPO_ROOT}/scripts/lib/cross-platform.sh"

case "$(uname -s)" in
    Linux)
        BACKEND_LABEL="crosvm"
        ;;
    Darwin)
        echo "[measure-crosvm-baseline] this script is the Linux/crosvm lane."
        echo "[measure-crosvm-baseline] on macOS, run scripts/measure-vz-baseline.sh instead."
        echo "[measure-crosvm-baseline] exiting cleanly (rc=0; not a failure)."
        exit 0
        ;;
    *)
        echo "[measure-crosvm-baseline] unsupported OS: $(uname -s)" >&2
        exit 1
        ;;
esac

# Phase 5 Day 6 — FORCE_FULL override (highest precedence).
if [[ "${ELASTOS_VZ_SMOKE_FORCE_FULL:-0}" == "1" ]]; then
    echo "[measure-crosvm-baseline] FORCE_FULL=1 — forcing full measurement run (overrides CI auto-detect)"
    export ELASTOS_VZ_SMOKE_DRY_RUN=0
fi

# Phase 5 Day 5 — auto-dry-run in CI.
if [[ -z "${ELASTOS_VZ_SMOKE_DRY_RUN:-}" ]] && cross_platform_in_ci; then
    echo "[measure-crosvm-baseline] CI detected (GITHUB_ACTIONS or CI env set); auto-enabling ELASTOS_VZ_SMOKE_DRY_RUN=1"
    export ELASTOS_VZ_SMOKE_DRY_RUN=1
fi

# Dry-run lane.
if [[ "${ELASTOS_VZ_SMOKE_DRY_RUN:-0}" == "1" ]]; then
    echo "[measure-crosvm-baseline] dry-run mode: parse OK, helper sourced OK; exiting before cargo test"
    exit 0
fi

ELASTOS_ROOT="${REPO_ROOT}/elastos"
TARGET_DIR="${ELASTOS_ROOT}/target"
REPORT_DIR="${TARGET_DIR}"
mkdir -p "${REPORT_DIR}"
JSONL_PATH="${REPORT_DIR}/crosvm-baseline.jsonl"
BASELINE_PATH="${REPORT_DIR}/crosvm-baseline.json"

RUNS="${ELASTOS_VZ_PERF_RUNS:-5}"
echo "[measure-crosvm-baseline] starting ${RUNS} runs (backend=${BACKEND_LABEL})"
echo "[measure-crosvm-baseline] JSONL → ${JSONL_PATH}"
echo "[measure-crosvm-baseline] aggregated → ${BASELINE_PATH}"

: > "${JSONL_PATH}"

run_idx=1
while [[ "${run_idx}" -le "${RUNS}" ]]; do
    echo "[measure-crosvm-baseline] run ${run_idx}/${RUNS}…"
    ELASTOS_VZ_PERF_REPORT="${JSONL_PATH}" cargo test \
        --manifest-path "${ELASTOS_ROOT}/Cargo.toml" \
        -p elastos-server \
        --test vz_perf_harness \
        -- --test-threads=1 \
        >/dev/null 2>&1 || {
            echo "[measure-crosvm-baseline] run ${run_idx} FAILED — aborting" >&2
            exit 1
        }
    run_idx=$((run_idx + 1))
done

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

# crosvm-lane notes: real microVM boot also not measured at
# Day 7. The wording differs from the Vz side because the
# crosvm-side blocker isn't release metadata — crosvm boots
# work locally today — but the *automated baseline* of those
# boots is a Phase-6 follow-up.
crosvm_notes = {
    "real_microvm_boot_measured": False,
    "real_microvm_boot_blocker":  "Phase 6 follow-up — synthetic harness today; real crosvm boots are reachable locally but not aggregated by this script.",
}

baseline = {
    "schema_version": schema_version or 1,
    "captured_at_unix_ms": int(time.time() * 1000),
    "host": host or {},
    "backend": backend_label,
    "notes": crosvm_notes,
    "metrics": metrics,
}

with open(baseline_path, "w") as f:
    json.dump(baseline, f, indent=2, sort_keys=True)
    f.write("\n")

print()
print(f"=== crosvm baseline ({backend_label}) ===")
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
print(f"  NOTE: real microVM boot timings NOT aggregated by this script (Day-7 synthetic harness).")
print()
PY

echo "[measure-crosvm-baseline] OK — baseline written to ${BASELINE_PATH}"
