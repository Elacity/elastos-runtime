#!/usr/bin/env bash
set -euo pipefail
umask 077
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
CI_HOME="${CI_SOURCE_HOME:-${HOME}/.elastos-ci/source-home}"
EVIDENCE="${RUNNER_TEMP}/source-home-journeys"
DATA="${CI_HOME}/.local/share/elastos"
[[ "$(uname -s)" != Darwin ]] || DATA="${CI_HOME}/Library/Application Support/elastos"
mkdir -p "$EVIDENCE"

case "${1:-}" in
    prepare)
        case "$(uname -s)-$(uname -m)" in
            Linux-x86_64) PLATFORM=linux-amd64 ;;
            Linux-aarch64|Linux-arm64) PLATFORM=linux-arm64 ;;
            Darwin-arm64) PLATFORM=darwin-arm64 ;;
            *) echo "Unsupported installed journey platform" >&2; exit 1 ;;
        esac
        # The existing seed contract verifies the declared archive checksum.
        # Linux source-home skips Kubo by default; macOS reuses its seeded cache.
        scripts/seed-kubo-cache.sh "${RUNNER_TEMP}/kubo-cache" "$DATA" "$PLATFORM"
        if [[ "$(uname -s)-$(uname -m)" == Linux-aarch64 ]]; then
            # #97's Carrier fixture already installs this pinned archive through
            # Runtime setup. Reuse its complete verified bundle and receipt.
            python3 - "$DATA" "${HOME}/.elastos-ci/carrier-fixture/xdg-data/elastos" <<'PY'
import json, shutil, sys
from pathlib import Path
from runpy import run_path
digest = run_path("scripts/ci-installed-journeys.py")["digest"]
data, fixture = map(Path, sys.argv[1:])
info = json.loads((data / "components.json").read_text())["external"]["llama-server"]["platforms"]["linux-arm64"]
bundle = fixture / info["install_path"]
receipt = json.loads((bundle / ".elastos-engine.json").read_text())
assert receipt["platform"] == "linux-arm64"
assert receipt["archive_sha256"] == info["checksum"]
binary = next(row for row in receipt["entries"] if row["path"] == "llama-server")
assert "sha256:" + digest(bundle / "llama-server") == binary["sha256"]
destination = data / info["install_path"]
destination.parent.mkdir(parents=True, exist_ok=True)
shutil.copytree(bundle, destination, symlinks=True)
PY
        fi
        # The existing verifier owns the pinned weight and license identities.
        scripts/pinned-model-consumer-proof.sh "${CI_HOME}/model-inputs" --fetch --check-inputs
        node scripts/ci-model-package.mjs "$DATA" "${CI_HOME}/model-inputs/inputs" "$EVIDENCE"
        ;;
    home)
        python3 scripts/ci-installed-journeys.py "$CI_HOME" "$DATA" "$EVIDENCE"
        ;;
    home-repeat)
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
runtime = data / "bin/elastos"
sha = digest(runtime) if runtime.is_file() else "unavailable: installation did not complete"
results = {"home_screenshots": "failed or not run", "model_package_admission": "failed or not run", "installed_runtime_reply": "failed or not run"}
elapsed = 0
records = []
paths = sorted(root.glob("run-*/installed-journeys.json")) or [root / "installed-journeys.json"]
for path in paths:
    if path.exists():
        row = json.loads(path.read_text())
        records.append(row)
        elapsed += row.get("elapsed_seconds", 0)
expected_runs = 3 if sys.platform == "darwin" else 1
for name in results:
    if len(records) == expected_runs and all(row.get("results", {}).get(name) == "passed" for row in records):
        results[name] = "passed"
spread = journey["TIMING"]["timing_spread"](records, expected_runs)
if any(row.get("candidate") != commit or row.get("installed_runtime_sha256") != sha for row in records):
    spread = {"status": "incomplete", "expected_runs": expected_runs, "recorded_runs": len(records)}
(root / "core-summary.json").write_text(json.dumps({
    "candidate": commit, "installed_runtime_sha256": sha,
    "results": results, "elapsed_seconds": elapsed, "model_timing_spread": spread,
    "fixture_policy": "fresh Home state and engine per run; same host with potentially warm OS file cache",
}, indent=2) + "\n")
with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
    summary.write(f"### Installed journeys\n\nCandidate: `{commit}`\n\nInstalled Runtime SHA-256: `{sha}`\n\n")
    summary.write("| Journey | Result |\n| --- | --- |\n")
    for name, result in results.items():
        summary.write(f"| {name} | {result} |\n")
    summary.write("\nEach run uses fresh Home state and a new engine process on the same host. OS file cache can warm between runs.\n")
    summary.write(f"\nTiming receipts: {spread['status']} ({spread['recorded_runs']}/{expected_runs} runs).\n")
    summary.write("\nEngine readiness starts at endpoint entry; other times start at worker entry. Generation excludes delta Applied waits and includes HTTP/stream handling. Applied waits include coordinator queue, reconciliation and durable storage.\n")
    if spread["status"] == "complete":
        summary.write("\n| Duration (ms) | Each run | Min | Max | Spread |\n| --- | --- | --- | --- | --- |\n")
        for name, item in spread["durations_ms"].items():
            summary.write(f"| {name} | {', '.join(map(str, item['values']))} | {item['min']} | {item['max']} | {item['spread']} |\n")
        summary.write("\n| Run | Delta Applied count | Longest delta wait (ms) | Terminal Applied count |\n| --- | --- | --- | --- |\n")
        for index, row in enumerate(records, 1):
            item = row["model_timing"]["durations"]
            summary.write(f"| {index} | {item['delta_acknowledgement_count']} | {item['delta_acknowledgement_max_ms']} | {item['terminal_acknowledgement_count']} |\n")
    summary.write(f"\nRecorded journey execution time: {elapsed} seconds. Package identity and screenshots are in the journey artifact.\n")
if spread["status"] != "complete":
    raise SystemExit("installed journey timing series is incomplete")
PY
        ;;
    *) echo "Usage: $0 prepare|home|home-repeat|summary" >&2; exit 2 ;;
esac
