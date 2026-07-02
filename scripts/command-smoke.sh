#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVER_MANIFEST="$ROOT/elastos/Cargo.toml"
ELASTOS_CMD=(cargo run -q -p elastos-server --manifest-path "$SERVER_MANIFEST" --)
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT
export XDG_DATA_HOME="$TMP_ROOT/xdg-data"
# macOS resolves the runtime data dir via `dirs::data_dir()` (~/Library/Application
# Support), which ignores XDG_DATA_HOME — so redirect HOME itself to keep the smoke
# hermetic, while pinning the toolchain caches to the real home so nothing re-downloads.
if [[ "$(uname -s)" == "Darwin" ]]; then
  REAL_HOME="$HOME"
  export CARGO_HOME="${CARGO_HOME:-$REAL_HOME/.cargo}"
  export RUSTUP_HOME="${RUSTUP_HOME:-$REAL_HOME/.rustup}"
  export HOME="$TMP_ROOT/home"
  mkdir -p "$HOME"
fi

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

# Portable timeout: stock macOS ships no GNU `timeout`; fall back to gtimeout
# (coreutils) or a perl alarm wrapper so fail-fast checks stay bounded (and cannot
# pass vacuously on `timeout: command not found`).
run_with_timeout() {
  local seconds="$1"
  shift
  if command -v timeout >/dev/null 2>&1; then
    timeout "${seconds}s" "$@"
  elif command -v gtimeout >/dev/null 2>&1; then
    gtimeout "${seconds}s" "$@"
  else
    perl -e 'alarm shift; exec @ARGV or die "exec failed: $!"' "$seconds" "$@"
  fi
}

run_fail_fast() {
  local name="$1"
  shift
  echo "[command-smoke] $name"
  set +e
  run_with_timeout 15 "$@" >/tmp/command-smoke.out 2>/tmp/command-smoke.err
  local rc=$?
  set -e
  if [[ $rc -eq 0 ]]; then
    echo "[command-smoke] expected failure for $name, but command succeeded" >&2
    cat /tmp/command-smoke.out >&2 || true
    cat /tmp/command-smoke.err >&2 || true
    exit 1
  fi
  if [[ $rc -eq 124 || $rc -eq 142 ]]; then
    echo "[command-smoke] command hung for $name" >&2
    cat /tmp/command-smoke.out >&2 || true
    cat /tmp/command-smoke.err >&2 || true
    exit 1
  fi
}

run_expect_output "root help exposes home" "home" "${ELASTOS_CMD[@]}" --help
run_expect_output "root help exposes webspace" "webspace" "${ELASTOS_CMD[@]}" --help
run_expect_output "root help exposes identity" "identity" "${ELASTOS_CMD[@]}" --help
run_ok "run help" "${ELASTOS_CMD[@]}" run --help
run_ok "home help" "${ELASTOS_CMD[@]}" home --help
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
  "${ELASTOS_CMD[@]}" run "$ROOT/capsules/home-cli"
run_fail_fast "open missing bundle CID" "${ELASTOS_CMD[@]}" open elastos://QmU8x9HMWetGzfnXLe4CriiocGuzvSLr9NJ1RwDp6MaWX6

echo "[command-smoke] OK"
