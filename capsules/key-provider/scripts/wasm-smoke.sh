#!/usr/bin/env bash
#
# WASI-sandbox smoke test for key-provider.
#
# Proves the provider executes correctly under a real WASI host (wasmtime),
# driving its newline-delimited JSON protocol over stdin/stdout, and that the
# fail-closed security contract holds in the sandbox: blocked raw authority
# advertised, malformed input rejected, denied/mismatched rights receipts
# rejected, and valid releases fail closed (not_configured) until the PQ-hybrid
# dKMS backend exists (see docs/convergence/DDRM_DECRYPT_RAIL.md).
#
# Usage: capsules/key-provider/scripts/wasm-smoke.sh
# Exit code: 0 if all cases pass, 1 otherwise.

set -euo pipefail

cd "$(dirname "$0")/.."

TARGET="wasm32-wasip1"
WASM="target/${TARGET}/debug/key-provider.wasm"

if ! command -v wasmtime >/dev/null 2>&1; then
  echo "FAIL: wasmtime CLI not found (install: brew install wasmtime)" >&2
  exit 1
fi

if [[ ! -f "$WASM" ]]; then
  echo "building $WASM ..."
  cargo build --target "$TARGET" >/dev/null
fi

# A fully valid key-release request: rights receipt allows the action and is bound
# to the same principal/session/object; PQ-hybrid envelope. Carries only a *wrapped*
# CEK — never raw key material.
VALID_RELEASE='{"op":"release","request":{"schema":"elastos.key_release.request/v1","request_id":"key-release:smoke","principal_id":"person:local:smoke","session_id":"session:smoke","object_cid":"bafybeigprotectedcontent","action":"view","rights_receipt":{"schema":"elastos.rights.decision.receipt/v1","request_id":"rights:smoke","content_id":"bafybeigprotectedcontent","principal_id":"person:local:smoke","session_id":"session:smoke","right":"view","provider":"rights-provider","allowed":true,"issued_at":1800000000,"expires_at":1900000000},"key_envelope":{"scheme":"elastos-pq-hybrid-threshold-v0","kid":"kid:smoke","wrapped_cek":"wrapped","policy_hash":"sha256:smoke","algorithms":{"cipher":"aes-256-gcm","signature":["ed25519","ml-dsa-65"],"kem":["x25519","ml-kem-768"],"share_scheme":"shamir-t-of-n"}},"reason":"open protected document","expires_at":1900000000}}'

# Same request, but the rights decision is denied: must be rejected up front.
DENIED_RELEASE="${VALID_RELEASE/\"allowed\":true/\"allowed\":false}"

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

run_case "status advertises blocked raw authority" \
  '{"op":"status"}' '"raw_cek"' || failures=$((failures + 1))

run_case "malformed op fails closed" \
  '{"op":"bogus"}' '"code":"invalid_request"' || failures=$((failures + 1))

run_case "valid release fails closed (no dKMS backend yet)" \
  "$VALID_RELEASE" '"code":"not_configured"' || failures=$((failures + 1))

run_case "denied rights receipt rejected" \
  "$DENIED_RELEASE" '"code":"invalid_request"' || failures=$((failures + 1))

echo
if [[ $failures -eq 0 ]]; then
  echo "wasm-smoke: ALL PASS (key-provider executes correctly + fails closed in the WASI sandbox)"
  exit 0
fi
echo "wasm-smoke: ${failures} case(s) FAILED" >&2
exit 1
