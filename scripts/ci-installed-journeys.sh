#!/usr/bin/env bash
set -euo pipefail
umask 077
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
CI_HOME="${CI_SOURCE_HOME:?Set CI_SOURCE_HOME to the receipt-bound source Home}"
EVIDENCE="${RUNNER_TEMP}/source-home-journeys"
DATA="${CI_HOME}/.local/share/elastos"
[[ "$(uname -s)" != Darwin ]] || DATA="${CI_HOME}/Library/Application Support/elastos"
export CI_MODEL_INPUTS="${CI_HOME}/model-inputs/inputs"
mkdir -p "$EVIDENCE"

case "${1:-}" in
    prepare)
        case "$(uname -s)-$(uname -m)" in
            Linux-x86_64) PLATFORM=linux-amd64 ;;
            Linux-aarch64|Linux-arm64) PLATFORM=linux-arm64 ;;
            Darwin-arm64) PLATFORM=darwin-arm64 ;;
            *) echo "Unsupported installed journey platform" >&2; exit 1 ;;
        esac
        # Source Homes prepare the engine through the current licensed recipe.
        # Kubo is installed before the source installation receipt is written.
        # Signed-release on-demand acquisition has a separate acceptance proof.
        python3 - "$DATA" "$PLATFORM" <<'PY'
from pathlib import Path
from runpy import run_path
import sys
journey = run_path("scripts/ci-installed-journeys.py")
data = Path(sys.argv[1]).resolve()
journey["require_disk_space"](data)
assert journey["engine_receipt"](data)["platform"] == sys.argv[2]
PY
        # The existing verifier owns the pinned weight and license identities.
        scripts/pinned-model-consumer-proof.sh "${CI_HOME}/model-inputs" --fetch --check-inputs
        ;;
    home)
        python3 scripts/ci-installed-journeys.py "$CI_HOME" "$DATA" "$EVIDENCE/engine-absent-home" --home-only
        python3 scripts/ci-installed-journeys.py "$CI_HOME" "$DATA" "$EVIDENCE"
        ;;
    home-repeat)
        python3 scripts/ci-installed-journeys.py "$CI_HOME" "$DATA" "$EVIDENCE/engine-absent-home" --home-only
        python3 scripts/ci-installed-journeys.py "$CI_HOME" "$DATA" "$EVIDENCE" --repeat 3
        ;;
    summary)
        python3 - "$EVIDENCE" "$DATA" <<'PY'
import json, os, subprocess, sys
from pathlib import Path
from runpy import run_path
journey = run_path("scripts/ci-installed-journeys.py")
digest = journey["digest"]
root, data = map(Path, sys.argv[1:])
commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
tree = subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], text=True).strip()
runtime = data / "bin/elastos"
sha = digest(runtime) if runtime.is_file() else "unavailable: installation did not complete"
provider = data / "bin/model-provider"
provider_sha = digest(provider) if provider.is_file() else "unavailable: provider installation did not complete"
source_manifest_sha = digest(data / "components.json") if (data / "components.json").is_file() else "unavailable"
try:
    engine = journey["engine_receipt"](data)
except (OSError, ValueError, AssertionError, KeyError, RuntimeError):
    engine = None
results = {name: "failed or not run" for name in (
    "home_screenshots", "model_package_admission", "installed_runtime_reply", "model_timing_observer", "process_cleanup", "disk_reserve")}
elapsed = 0
records = []
paths = sorted(root.glob("run-*/installed-journeys.json")) or [root / "installed-journeys.json"]
for path in paths:
    if path.exists():
        row = json.loads(path.read_text())
        records.append(row)
        elapsed += row.get("elapsed_seconds", 0) + row.get("fixture_preparation_seconds", 0)
expected_runs = 3 if sys.platform == "darwin" else 1
for name in results:
    if len(records) == expected_runs and all(row.get("results", {}).get(name) == "passed" for row in records):
        results[name] = "passed"
spread = (journey["TIMING"]["timing_spread"](records, expected_runs) if sys.platform == "darwin" else
          {"status": "unavailable_with_confined_provider_stderr", "expected_runs": expected_runs, "recorded_runs": len(records)})
current = lambda row: (row.get("candidate") == commit and row.get("source_tree") == tree
                       and row.get("installed_runtime_sha256") == sha
                       and row.get("installed_model_provider_sha256") == provider_sha
                       and row.get("source_components_sha256") == source_manifest_sha)
if len(records) != expected_runs or engine is None or any(not current(row) or row.get("installed_engine") != engine for row in records):
    spread = {"status": "incomplete", "expected_runs": expected_runs, "recorded_runs": len(records)}
absence_path = root / "engine-absent-home/installed-journeys.json"
absence = json.loads(absence_path.read_text()) if absence_path.exists() else {}
results["engine_absent_home"] = ("passed" if current(absence) and absence.get("engine_absent") is True
                                and absence.get("dispatch_unavailable_reason") == "source_engine_required"
                                and absence.get("process_cleanup", {}).get("before", {}).get("llama_server") == 0
                                and all(absence.get("results", {}).get(name) == "passed" for name in
                                        ("engine_absent_home", "engine_absent_refusal", "home_screenshots", "process_cleanup", "disk_reserve"))
                                else "failed or not run")
elapsed += absence.get("elapsed_seconds", 0) + absence.get("fixture_preparation_seconds", 0)
(root / "core-summary.json").write_text(json.dumps({
    "candidate": commit, "source_tree": tree, "installed_runtime_sha256": sha,
    "results": results, "elapsed_seconds": elapsed, "model_timing_spread": spread,
    "fixture_policy": "fresh Home state and engine process per run; verified source engine prerequisite; same host with potentially warm OS file cache",
    "engine_absent_home": absence,
}, indent=2) + "\n")
with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
    summary.write(f"### Installed journeys\n\nCandidate: `{commit}`\n\nInstalled Runtime SHA-256: `{sha}`\n\n")
    summary.write("| Journey | Result |\n| --- | --- |\n")
    for name, result in results.items():
        summary.write(f"| {name} | {result} |\n")
    summary.write("\nEach run uses fresh Home state and a new engine process on the same host. OS file cache can warm between runs.\n")
    summary.write("\nThe reply fixture uses the verified source engine prerequisite. Signed-release engine acquisition has separate acceptance evidence. A separate fresh signed-fixture Home proves Get, controlled Use refusal, Retry and Home usability with the optional engine absent.\n")
    summary.write(f"\nTiming receipts: {spread['status']} ({spread['recorded_runs']}/{expected_runs} runs).\n")
    if spread["status"] == "complete":
        summary.write("\nEngine readiness starts at endpoint entry; other times start at worker entry. Generation excludes delta Applied waits and includes HTTP/stream handling. Applied waits include coordinator queue, reconciliation and durable storage.\n")
        summary.write("\n| Duration (ms) | Each run | Min | Max | Spread |\n| --- | --- | --- | --- | --- |\n")
        for name, item in spread["durations_ms"].items():
            summary.write(f"| {name} | {', '.join(map(str, item['values']))} | {item['min']} | {item['max']} | {item['spread']} |\n")
        summary.write("\n| Run | Delta Applied count | Longest delta wait (ms) | Terminal Applied count |\n| --- | --- | --- | --- |\n")
        for index, row in enumerate(records, 1):
            item = row["model_timing"]["durations"]
            summary.write(f"| {index} | {item['delta_acknowledgement_count']} | {item['delta_acknowledgement_max_ms']} | {item['terminal_acknowledgement_count']} |\n")
    summary.write(f"\nRecorded journey execution time: {elapsed} seconds. Package identity and screenshots are in the journey artifact.\n")
if spread["status"] == "incomplete" or any(result != "passed" for result in results.values()):
    raise SystemExit("installed journey acceptance is incomplete")
PY
        ;;
    *) echo "Usage: $0 prepare|home|home-repeat|summary" >&2; exit 2 ;;
esac
