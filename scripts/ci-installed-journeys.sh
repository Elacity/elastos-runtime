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
        if [[ "$(uname -s)-$(uname -m)" == Linux-aarch64 ]]; then
            # #97's Carrier fixture already installs this pinned archive through
            # Runtime setup. Reuse its complete verified bundle and receipt.
            python3 - "$DATA" "${HOME}/.elastos-ci/carrier-fixture/xdg-data/elastos" <<'PY'
import hashlib, json, shutil, sys
from pathlib import Path
data, fixture = map(Path, sys.argv[1:])
info = json.loads((data / "components.json").read_text())["external"]["llama-server"]["platforms"]["linux-arm64"]
bundle = fixture / info["install_path"]
receipt = json.loads((bundle / ".elastos-engine.json").read_text())
assert receipt["platform"] == "linux-arm64"
assert receipt["archive_sha256"] == info["checksum"]
binary = next(row for row in receipt["entries"] if row["path"] == "llama-server")
assert "sha256:" + hashlib.file_digest((bundle / "llama-server").open("rb"), "sha256").hexdigest() == binary["sha256"]
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
        if [[ "$(uname -s)" == Linux ]]; then
            sudo systemd-run --quiet --wait --pipe --collect \
                --unit="elastos-ci-home-${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT}" \
                --uid="$(id -u)" --gid="$(id -g)" \
                -p MemoryMax=4G -p MemorySwapMax=0 -p TasksMax=512 \
                -p "WorkingDirectory=${ROOT}" \
                /usr/bin/env "PATH=${PATH}" "HOME=${HOME}" \
                "PLAYWRIGHT_BROWSERS_PATH=${PLAYWRIGHT_BROWSERS_PATH}" \
                python3 scripts/ci-installed-journeys.py "$CI_HOME" "$DATA" "$EVIDENCE"
        else
            python3 scripts/ci-installed-journeys.py "$CI_HOME" "$DATA" "$EVIDENCE"
        fi
        ;;
    lifecycle)
        # Reuse #98's accepted production process fixture, with the binaries
        # installed by this job. Compile outside the finite resource scope.
        rg -q 'fn installed_local_resource_lifecycle\(' capsules/model-provider/tests/process.rs || {
            echo "LA-04 lifecycle requires the integrated #98 fixture" >&2; exit 1;
        }
        cargo test --locked --release --manifest-path capsules/model-provider/Cargo.toml \
            --test process --no-run --message-format=json >"${CI_HOME}/resource-test-build.private.jsonl"
        TEST_BIN="$(python3 - "${CI_HOME}/resource-test-build.private.jsonl" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    row = json.loads(line)
    if row.get("reason") == "compiler-artifact" and row.get("target", {}).get("name") == "process" and row.get("executable"):
        print(row["executable"])
PY
)"
        [[ -x "$TEST_BIN" ]]
        FIXTURE="${CI_HOME}/resource-lifecycle"
        mkdir -p "${FIXTURE}/bin"
        install -m 700 "${DATA}/bin/model-provider" "${FIXTURE}/bin/model-provider"
        ENGINE="$(python3 - "$DATA" <<'PY'
import json, platform, sys
from pathlib import Path
data = Path(sys.argv[1])
host = "linux-arm64" if platform.machine() in ("aarch64", "arm64") else "linux-amd64"
print(data / json.loads((data / "components.json").read_text())["external"]["llama-server"]["platforms"][host]["install_path"])
PY
)"
        for name in first second; do
            mkdir -p "${FIXTURE}/${name}"
            cp -R "$ENGINE" "${FIXTURE}/${name}/engine"
            install -m 400 "${CI_HOME}/model-inputs/inputs/SmolLM2-135M-Instruct-Q8_0.gguf" "${FIXTURE}/${name}/model.gguf"
        done
        for qualification in low-memory lifecycle; do
            limit=4G; low_memory=0
            [[ "$qualification" != low-memory ]] || { limit=1G; low_memory=1; }
            sudo systemd-run --quiet --wait --pipe --collect \
            --unit="elastos-ci-la04-${qualification}-${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT}" \
            --uid="$(id -u)" --gid="$(id -g)" \
            -p "MemoryMax=${limit}" -p MemorySwapMax=0 -p TasksMax=512 \
            -p "WorkingDirectory=${ROOT}" \
            /usr/bin/env "PATH=${PATH}" "HOME=${CI_HOME}" \
            "ELASTOS_MODEL_RESOURCE_PROOF_ROOT=${FIXTURE}" \
            "ELASTOS_MODEL_RESOURCE_PROOF_LOW_MEMORY=${low_memory}" \
            bash -ec 'python3 - "$1" <<'"'"'PY'"'"'
import json, sys
sys.path.insert(0, "scripts")
from importlib.machinery import SourceFileLoader
module = SourceFileLoader("journeys", "scripts/ci-installed-journeys.py").load_module()
open(sys.argv[1], "w").write(json.dumps(module.cgroup_observation(), indent=2))
PY
"$2" installed_local_resource_lifecycle --ignored --exact --nocapture' \
            fixture "${EVIDENCE}/la04-${qualification}-cgroup.json" "$TEST_BIN" 2>&1 | tee "${EVIDENCE}/la04-${qualification}.log"
        done
        python3 - "$EVIDENCE" <<'PY'
import json, sys
from pathlib import Path
root = Path(sys.argv[1])
for qualification in ["low-memory", "lifecycle"]:
    text = (root / f"la04-{qualification}.log").read_text()
    assert text.count("test installed_local_resource_lifecycle ... ok") == 1
    assert "test result: ok. 1 passed; 0 failed; 0 ignored;" in text
(root / "la04-result.json").write_text(json.dumps({"la04_low_memory": "passed", "la04_lifecycle": "passed"}))
PY
        ;;
    summary)
        python3 - "$EVIDENCE" "$DATA" <<'PY'
import hashlib, json, os, subprocess, sys
from pathlib import Path
root, data = map(Path, sys.argv[1:])
commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
sha = hashlib.file_digest((data / "bin/elastos").open("rb"), "sha256").hexdigest()
results = {"home_screenshots": "failed or not run", "model_package_admission": "failed or not run", "installed_runtime_reply": "failed or not run", "la04_low_memory": "not required on macOS" if sys.platform == "darwin" else "failed or not run", "la04_lifecycle": "not required on macOS" if sys.platform == "darwin" else "failed or not run"}
elapsed = None
for name in ["installed-journeys.json", "la04-result.json"]:
    if (root / name).exists():
        row = json.loads((root / name).read_text())
        results.update(row.get("results", row if name == "la04-result.json" else {}))
        elapsed = row.get("elapsed_seconds", elapsed)
with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
    summary.write(f"### Installed journeys\n\nCandidate: `{commit}`\n\nInstalled Runtime SHA-256: `{sha}`\n\n")
    summary.write("| Journey | Result |\n| --- | --- |\n")
    for name, result in results.items():
        summary.write(f"| {name} | {result} |\n")
    summary.write(f"\nHome journey time: {elapsed} seconds. Cgroup ancestry, finite limits, package identity and screenshots are in the journey artifact.\n")
PY
        ;;
    *) echo "Usage: $0 prepare|home|lifecycle|summary" >&2; exit 2 ;;
esac
