#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

tmp_dir="$(mktemp -d /tmp/elastos-mac-source-home-restart-smoke-XXXXXX)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

dry_run_json="$tmp_dir/restart-dry-run.json"
"$repo_root/scripts/mac-source-home-restart.sh" \
  --dry-run \
  --test-home "$tmp_dir/test home" \
  --addr localhost:61180 \
  --log-dir "$tmp_dir/logs" \
  --json-out "$dry_run_json" \
  >"$tmp_dir/stdout.json"

python3 - "$repo_root" "$dry_run_json" "$tmp_dir/stdout.json" "$tmp_dir/test home" <<'PY'
import json
import pathlib
import sys

repo, json_path, stdout_path, test_home = sys.argv[1:]
from_file = json.loads(pathlib.Path(json_path).read_text())
from_stdout = json.loads(pathlib.Path(stdout_path).read_text())
if from_file != from_stdout:
    raise SystemExit("dry-run stdout and --json-out differ")
if from_file.get("schema") != "elastos.mac-source-home-restart/v1":
    raise SystemExit("unexpected restart schema")
if from_file.get("ok") is not True or from_file.get("dry_run") is not True:
    raise SystemExit("dry-run restart plan must be ok=true and dry_run=true")
if not isinstance(from_file.get("generated_at"), str) or not from_file["generated_at"].endswith("Z"):
    raise SystemExit("dry-run restart plan generated_at mismatch")
if from_file.get("repo") != repo:
    raise SystemExit("restart plan repo mismatch")
if from_file.get("test_home") != test_home:
    raise SystemExit("restart plan test_home mismatch")
if from_file.get("data_dir") != f"{test_home}/Library/Application Support/elastos":
    raise SystemExit("restart plan data_dir mismatch")
if from_file.get("home_url") != "http://localhost:61180/apps/home/":
    raise SystemExit("restart plan home URL mismatch")
if not from_file.get("gateway_bin", "").endswith("/elastos/target/release/elastos"):
    raise SystemExit("restart plan gateway binary mismatch")
if not from_file.get("gateway_log", "").startswith(f"{pathlib.Path(sys.argv[4]).parent}/logs/gateway-"):
    raise SystemExit("restart plan gateway log mismatch")
PY

python3 - "$repo_root/scripts/mac-source-home-restart.sh" <<'PY'
import pathlib
import sys

source = pathlib.Path(sys.argv[1]).read_text()
required = [
    "verify_browser_helper_freshness",
    "browser_helper_source_sha256",
    "browser_helper_installed_sha256",
    "browser_helper_initrd_sha256",
    "browser_helper_rootfs_sha256",
    "Mac source-home Browser helper verification failed",
]
missing = [needle for needle in required if needle not in source]
if missing:
    raise SystemExit(f"restart helper lost Browser freshness gate: {missing}")
PY

set +e
"$repo_root/scripts/mac-source-home-restart.sh" --dry-run --addr not-a-host-port \
  >"$tmp_dir/invalid-addr.out" \
  2>"$tmp_dir/invalid-addr.err"
invalid_addr_status=$?
set -e
if [[ "$invalid_addr_status" -eq 0 ]]; then
  echo "mac-source-home-restart accepted an invalid address" >&2
  exit 1
fi

printf '{"schema":"elastos.mac-source-home-restart-smoke/v1","ok":true,"dry_run_contract":true,"browser_helper_freshness_gate_present":true,"invalid_addr_rejected":true}\n'
