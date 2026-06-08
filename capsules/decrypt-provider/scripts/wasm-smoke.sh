#!/usr/bin/env bash
#
# WASI-sandbox smoke test for decrypt-provider.
#
# Proves the provider does not just compile to wasm32-wasip1 but *executes*
# correctly under a real WASI host (wasmtime), driving its newline-delimited JSON
# protocol over stdin/stdout. Asserts the fail-closed security contract holds in
# the sandbox: blocked raw-authority advertised, malformed input rejected, and
# decrypt sessions fail closed until the key/ciphertext rail lands
# (see docs/convergence/DDRM_DECRYPT_RAIL.md).
#
# Usage: capsules/decrypt-provider/scripts/wasm-smoke.sh
# Exit code: 0 if all cases pass, 1 otherwise.

set -euo pipefail

cd "$(dirname "$0")/.."

TARGET="wasm32-wasip1"
WASM="target/${TARGET}/debug/decrypt-provider.wasm"

if ! command -v wasmtime >/dev/null 2>&1; then
  echo "FAIL: wasmtime CLI not found (install: brew install wasmtime)" >&2
  exit 1
fi

if [[ ! -f "$WASM" ]]; then
  echo "building $WASM ..."
  cargo build --target "$TARGET" >/dev/null
fi

# A fully valid decrypt session request (authority + intent only; no key/ciphertext).
VALID_SESSION='{"op":"open_session","request":{"schema":"elastos.decrypt.session.request/v1","request_id":"decrypt:smoke","principal_id":"person:local:smoke","session_id":"session:smoke","object_cid":"bafybeigprotectedcontent","action":"view","viewer_interface":"elastos.viewer/document@1","release_receipt":{"schema":"elastos.release.receipt/v1","request_id":"key-release:smoke","object_cid":"bafybeigprotectedcontent","principal_id":"person:local:smoke","session_id":"session:smoke","action":"view","provider":"key-provider","status":"released","issued_at":1800000000,"expires_at":1900000000},"output_kind":"rendered","reason":"open protected document","expires_at":1900000000}}'

# A session whose output_kind asks for raw plaintext: must be rejected up front.
RAW_PLAINTEXT_SESSION="${VALID_SESSION/\"output_kind\":\"rendered\"/\"output_kind\":\"raw_plaintext\"}"

run_case() {
  local label="$1" input="$2" expect="$3"
  local output
  output="$(printf '%s\n' "$input" | wasmtime run "$WASM" 2>/dev/null)"
  echo "── ${label}"
  echo "   IN : ${input:0:80}$([[ ${#input} -gt 80 ]] && echo '…')"
  echo "   OUT: ${output}"
  if [[ "$output" == *"$expect"* ]]; then
    echo "   PASS (matched: ${expect})"
    return 0
  fi
  echo "   FAIL (expected substring: ${expect})"
  return 1
}

failures=0

run_case "status advertises blocked raw authority" \
  '{"op":"status"}' '"raw_cek"' || failures=$((failures + 1))

run_case "malformed op fails closed" \
  '{"op":"bogus"}' '"code":"invalid_request"' || failures=$((failures + 1))

run_case "valid session fails closed (no rail yet)" \
  "$VALID_SESSION" '"code":"not_configured"' || failures=$((failures + 1))

run_case "raw_plaintext output_kind rejected before backend" \
  "$RAW_PLAINTEXT_SESSION" '"code":"invalid_request"' || failures=$((failures + 1))

echo
if [[ $failures -eq 0 ]]; then
  echo "wasm-smoke: ALL PASS (decrypt-provider executes correctly + fails closed in the WASI sandbox)"
  exit 0
fi
echo "wasm-smoke: ${failures} case(s) FAILED" >&2
exit 1
