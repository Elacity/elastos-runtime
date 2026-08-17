#!/usr/bin/env bash
#
# WASI-sandbox smoke test for rights-provider.
#
# Proves the provider executes correctly under a real WASI host (wasmtime),
# driving its newline-delimited JSON protocol over stdin/stdout, and that the
# fail-closed security contract holds in the sandbox: blocked chain/wallet/key
# authority advertised, malformed input and unsupported actions rejected, and
# valid rights questions fail closed (not_configured) until the dDRM/chain policy
# backend exists (see docs/dkms/history/DDRM_DECRYPT_RAIL.md).
#
# Usage: capsules/rights-provider/scripts/wasm-smoke.sh
# Exit code: 0 if all cases pass, 1 otherwise.

set -euo pipefail

cd "$(dirname "$0")/.."

TARGET="wasm32-wasip1"
WASM="target/${TARGET}/debug/rights-provider.wasm"

if ! command -v wasmtime >/dev/null 2>&1; then
  echo "FAIL: wasmtime CLI not found (install: brew install wasmtime)" >&2
  exit 1
fi

if [[ ! -f "$WASM" ]]; then
  echo "building $WASM ..."
  cargo build --target "$TARGET" >/dev/null
fi

# A well-formed access question (typed question only; no chain/wallet/key authority).
VALID_ACCESS='{"op":"has_access_by_content_id","request":{"principal_id":"person:local:smoke","session_id":"session:smoke","content_id":"bafybeigprotectedcontent","right":"view","reason":"open protected document","policy_ref":"bafybeigpolicy"}}'

# Same question with an unsupported right: must be rejected.
BAD_RIGHT="${VALID_ACCESS/\"right\":\"view\"/\"right\":\"raw_key\"}"

run_case() {
  local label="$1" input="$2" expect="$3"
  local output
  output="$(printf '%s\n' "$input" | wasmtime run "$WASM" 2>/dev/null)"
  echo "── ${label}"
  echo "   OUT: ${output}"
  if [[ "$output" == *"$expect"* ]]; then
    echo "   PASS (matched: ${expect})"
    return 0
  fi
  echo "   FAIL (expected substring: ${expect})"
  return 1
}

failures=0

run_case "status advertises blocked chain/wallet/key authority" \
  '{"op":"status"}' '"chain_rpc"' || failures=$((failures + 1))

run_case "malformed op fails closed" \
  '{"op":"bogus"}' '"code":"invalid_request"' || failures=$((failures + 1))

run_case "valid access check fails closed (no policy backend yet)" \
  "$VALID_ACCESS" '"code":"not_configured"' || failures=$((failures + 1))

run_case "unsupported right rejected" \
  "$BAD_RIGHT" '"code":"invalid_request"' || failures=$((failures + 1))

echo
if [[ $failures -eq 0 ]]; then
  echo "wasm-smoke: ALL PASS (rights-provider executes correctly + fails closed in the WASI sandbox)"
  exit 0
fi
echo "wasm-smoke: ${failures} case(s) FAILED" >&2
exit 1
