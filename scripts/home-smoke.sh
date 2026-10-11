#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "[home-smoke] building Home CLI renderer"
cargo build --manifest-path "$REPO_ROOT/capsules/home-cli/Cargo.toml" --release --bin home-cli >/dev/null

cd "$REPO_ROOT/elastos"

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

run_check() {
  local label="$1"
  local command="$2"
  local input="$3"
  local pattern="$4"

  : >"$tmp"
  CARGO_TERM_COLOR=never RUST_LOG=warn \
    printf "%b" "$input" | cargo run -p elastos-server -- ${command} >"$tmp" 2>&1

  if ! grep -q -E "$pattern" "$tmp"; then
    echo "[home-smoke] FAILED: $label"
    cat "$tmp"
    exit 1
  fi

  echo "[home-smoke] ok: $label"
}

run_check "default elastos opens Home" "" "q\n" "ElastOS Home"
run_check "chat returns home" "home" "1\n/home\nq\n" "Returned home from Chat\\."

echo "[home-smoke] OK"
