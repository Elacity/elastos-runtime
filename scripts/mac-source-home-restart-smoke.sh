#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

tmp_dir="$(mktemp -d /tmp/elastos-mac-source-home-restart-smoke-XXXXXX)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

dry_run_json="$tmp_dir/restart-dry-run.json"
run_dry_run() {
  local tag="$1"
  local cargo_target_dir="$2"
  local json_out="$tmp_dir/${tag}.json"
  local stdout_out="$tmp_dir/${tag}.stdout.json"
  local test_home="$tmp_dir/test home"
  if [[ -n "$cargo_target_dir" ]]; then
    CARGO_TARGET_DIR="$cargo_target_dir" \
      "$repo_root/scripts/mac-source-home-restart.sh" \
      --dry-run \
      --test-home "$test_home" \
      --addr localhost:61180 \
      --log-dir "$tmp_dir/logs" \
      --json-out "$json_out" \
      >"$stdout_out"
  else
    "$repo_root/scripts/mac-source-home-restart.sh" \
      --dry-run \
      --test-home "$test_home" \
      --addr localhost:61180 \
      --log-dir "$tmp_dir/logs" \
      --json-out "$json_out" \
      >"$stdout_out"
  fi
}

run_dry_run default ""
run_dry_run absolute "$tmp_dir/cargo-target-absolute"
run_dry_run relative "cargo-target-relative"

python3 - "$repo_root" "$tmp_dir" "$tmp_dir/test home" <<'PY'
import json
import pathlib
import sys

repo, tmp_dir, test_home = sys.argv[1:]
tmp_path = pathlib.Path(tmp_dir)

def load(tag):
    from_file = json.loads((tmp_path / f"{tag}.json").read_text())
    from_stdout = json.loads((tmp_path / f"{tag}.stdout.json").read_text())
    if from_file != from_stdout:
        raise SystemExit(f"{tag}: dry-run stdout and --json-out differ")
    return from_file

def assert_common(plan, tag):
    if plan.get("schema") != "elastos.mac-source-home-restart/v1":
        raise SystemExit(f"{tag}: unexpected restart schema")
    if plan.get("ok") is not True or plan.get("dry_run") is not True:
        raise SystemExit(f"{tag}: dry-run restart plan must be ok=true and dry_run=true")
    if not isinstance(plan.get("generated_at"), str) or not plan["generated_at"].endswith("Z"):
        raise SystemExit(f"{tag}: dry-run restart plan generated_at mismatch")
    if plan.get("repo") != repo:
        raise SystemExit(f"{tag}: restart plan repo mismatch")
    if plan.get("test_home") != test_home:
        raise SystemExit(f"{tag}: restart plan test_home mismatch")
    if plan.get("data_dir") != f"{test_home}/Library/Application Support/elastos":
        raise SystemExit(f"{tag}: restart plan data_dir mismatch")
    if plan.get("home_url") != "http://localhost:61180/apps/home/":
        raise SystemExit(f"{tag}: restart plan home URL mismatch")
    if not plan.get("gateway_log", "").startswith(f"{pathlib.Path(test_home).parent}/logs/gateway-"):
        raise SystemExit(f"{tag}: restart plan gateway log mismatch")

plans = {
    "default": load("default"),
    "absolute": load("absolute"),
    "relative": load("relative"),
}

for tag, plan in plans.items():
    assert_common(plan, tag)

expected = {
    "default": pathlib.Path(repo) / "elastos/target/release/elastos",
    "absolute": tmp_path / "cargo-target-absolute/release/elastos",
    "relative": pathlib.Path(repo) / "cargo-target-relative/release/elastos",
}
for tag, expected_gateway in expected.items():
    actual_gateway = pathlib.Path(plans[tag].get("gateway_bin", ""))
    if actual_gateway != expected_gateway:
        raise SystemExit(
            f"{tag}: restart plan gateway binary mismatch: {actual_gateway} != {expected_gateway}"
        )
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
    "principal-root-upgrade",
    "principal_root_backup_dir",
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
