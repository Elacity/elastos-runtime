#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "[pc2-smoke] building pc2 wasm capsule"
(
  cd "$REPO_ROOT/capsules/pc2"
  cargo build --target wasm32-wasip1 --release >/dev/null
)

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
    echo "[pc2-smoke] FAILED: $label"
    cat "$tmp"
    exit 1
  fi

  echo "[pc2-smoke] ok: $label"
}

run_check "default elastos opens PC2 home" "" "q\n" "ElastOS PC2"
run_check "chat returns home" "pc2" "1\n/home\nq\n" "Returned home from Chat\\."
run_check "mywebsite shows next-step notice" "pc2" "2\n\nq\n" "MyWebSite is empty\\.|MyWebSite is staged at localhost://MyWebSite\\.|MyWebSite is not ready: missing site-provider — run: elastos setup --profile demo"
run_check "updates action returns home" "pc2" "3\nq\n" "Returned home from Updates\\.|Updates:|Updates could not complete the trusted-source check:.*You are back at PC2 home\\."

echo "[pc2-smoke] OK"
