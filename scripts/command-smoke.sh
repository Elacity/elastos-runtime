#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVER_MANIFEST="$ROOT/elastos/Cargo.toml"
ELASTOS_CMD=(cargo run -q -p elastos-server --manifest-path "$SERVER_MANIFEST" --)
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT
export XDG_DATA_HOME="$TMP_ROOT/xdg-data"

run_ok() {
  local name="$1"
  shift
  echo "[command-smoke] $name"
  "$@" >/tmp/command-smoke.out 2>/tmp/command-smoke.err
}

run_expect_output() {
  local name="$1"
  local pattern="$2"
  shift 2
  echo "[command-smoke] $name"
  "$@" >/tmp/command-smoke.out 2>/tmp/command-smoke.err
  if ! grep -Eq "$pattern" /tmp/command-smoke.out /tmp/command-smoke.err; then
    echo "[command-smoke] expected pattern '$pattern' not found for $name" >&2
    cat /tmp/command-smoke.out >&2 || true
    cat /tmp/command-smoke.err >&2 || true
    exit 1
  fi
}

run_expect_failure_output() {
  local name="$1"
  local pattern="$2"
  shift 2
  echo "[command-smoke] $name"
  set +e
  "$@" >/tmp/command-smoke.out 2>/tmp/command-smoke.err
  local rc=$?
  set -e
  if [[ $rc -eq 0 ]]; then
    echo "[command-smoke] expected failure for $name, but command succeeded" >&2
    cat /tmp/command-smoke.out >&2 || true
    cat /tmp/command-smoke.err >&2 || true
    exit 1
  fi
  if ! grep -Eq "$pattern" /tmp/command-smoke.out /tmp/command-smoke.err; then
    echo "[command-smoke] expected pattern '$pattern' not found for $name" >&2
    cat /tmp/command-smoke.out >&2 || true
    cat /tmp/command-smoke.err >&2 || true
    exit 1
  fi
}

run_fail_fast() {
  local name="$1"
  shift
  echo "[command-smoke] $name"
  set +e
  timeout 15s "$@" >/tmp/command-smoke.out 2>/tmp/command-smoke.err
  local rc=$?
  set -e
  if [[ $rc -eq 0 ]]; then
    echo "[command-smoke] expected failure for $name, but command succeeded" >&2
    cat /tmp/command-smoke.out >&2 || true
    cat /tmp/command-smoke.err >&2 || true
    exit 1
  fi
  if [[ $rc -eq 124 ]]; then
    echo "[command-smoke] command hung for $name" >&2
    cat /tmp/command-smoke.out >&2 || true
    cat /tmp/command-smoke.err >&2 || true
    exit 1
  fi
}

run_expect_output "root help exposes pc2" "pc2" "${ELASTOS_CMD[@]}" --help
run_expect_output "root help exposes webspace" "webspace" "${ELASTOS_CMD[@]}" --help
run_expect_output "root help exposes identity" "identity" "${ELASTOS_CMD[@]}" --help
run_ok "run help" "${ELASTOS_CMD[@]}" run --help
run_ok "pc2 help" "${ELASTOS_CMD[@]}" pc2 --help
run_ok "identity help" "${ELASTOS_CMD[@]}" identity --help
run_ok "identity nickname help" "${ELASTOS_CMD[@]}" identity nickname --help
run_ok "webspace help" "${ELASTOS_CMD[@]}" webspace --help
run_ok "site help" "${ELASTOS_CMD[@]}" site --help
run_ok "site publish help" "${ELASTOS_CMD[@]}" site publish --help
run_ok "site activate help" "${ELASTOS_CMD[@]}" site activate --help
run_ok "site channels help" "${ELASTOS_CMD[@]}" site channels --help
run_expect_output "config show on empty home is explicit" "No config file found" "${ELASTOS_CMD[@]}" config show
run_expect_output "site path prints rooted path" "localhost://MyWebSite" "${ELASTOS_CMD[@]}" site path
run_expect_output "shares list on empty home is explicit" "No shares yet" "${ELASTOS_CMD[@]}" shares list
run_expect_failure_output \
  "run wasm without operator runtime fails clearly" \
  "This command requires a running runtime" \
  "${ELASTOS_CMD[@]}" run "$ROOT/capsules/pc2"
run_fail_fast "open missing bundle CID" "${ELASTOS_CMD[@]}" open elastos://QmU8x9HMWetGzfnXLe4CriiocGuzvSLr9NJ1RwDp6MaWX6

echo "[command-smoke] OK"
