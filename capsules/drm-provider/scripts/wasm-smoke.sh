#!/usr/bin/env bash
#
# WASI-sandbox smoke test for drm-provider (the drm/open orchestrator).
#
# Proves the chain's front door executes correctly under a real WASI host
# (wasmtime) and that the fail-closed contract holds in the sandbox: blocked raw
# authority advertised, the canonical open sequence declared, malformed input and
# non-sealed objects rejected, and a valid open emits ONLY the executable
# DrmOpenPlanV1 (Day 67: `status: planned` — the plan the runtime host executes)
# with NO session and NO key material; the rights/key/decrypt providers do the
# actual work (see docs/dkms/history/DDRM_DECRYPT_RAIL.md).
#
# Usage: capsules/drm-provider/scripts/wasm-smoke.sh
# Exit code: 0 if all cases pass, 1 otherwise.

set -euo pipefail

cd "$(dirname "$0")/.."

TARGET="wasm32-wasip1"
WASM="target/${TARGET}/debug/drm-provider.wasm"

if ! command -v wasmtime >/dev/null 2>&1; then
  echo "FAIL: wasmtime CLI not found (install: brew install wasmtime)" >&2
  exit 1
fi

if [[ ! -f "$WASM" ]]; then
  echo "building $WASM ..."
  cargo build --target "$TARGET" >/dev/null
fi

# A well-formed drm/open request over a PQ-hybrid sealed object (wrapped CEK only).
VALID_OPEN='{"op":"open","request":{"object":{"schema":"elastos.sealed.object/v1","payload_cid":"bafybeigpayload","rights_policy_cid":"bafybeigpolicy","availability_receipt_cid":"bafybeigreceipt","key_envelope":{"scheme":"elastos-pq-hybrid-threshold-v0","kid":"kid:smoke","wrapped_cek":"wrapped","policy_hash":"sha256:smoke","algorithms":{"cipher":"aes-256-gcm","signature":["ed25519","ml-dsa-65"],"kem":["x25519","ml-kem-768"],"share_scheme":"shamir-t-of-n"}},"viewer":{"required_interface":"elastos.viewer/document@1"}},"principal_id":"person:local:smoke","session_id":"session:smoke","action":"view","reason":"open protected document"}}'

# Same request but the object is not a sealed object: must be rejected up front.
NON_SEALED="${VALID_OPEN/\"schema\":\"elastos.sealed.object\/v1\"/\"schema\":\"elastos.object\/v1\"}"

run_case() {
  local label="$1" input="$2" expect="$3"
  local output
  output="$(printf '%s\n' "$input" | wasmtime run "$WASM" 2>/dev/null)"
  echo "── ${label}"
  echo "   OUT: ${output:0:200}$([[ ${#output} -gt 200 ]] && echo '…')"
  if [[ "$output" == *"$expect"* ]]; then
    echo "   PASS (matched: ${expect})"
    return 0
  fi
  echo "   FAIL (expected substring: ${expect})"
  return 1
}

failures=0

run_case "status declares canonical open sequence" \
  '{"op":"status"}' 'elastos://key/release' || failures=$((failures + 1))

run_case "malformed op fails closed" \
  '{"op":"bogus"}' '"code":"invalid_request"' || failures=$((failures + 1))

# Day 67+: a valid open is fail-closed by SHAPE — the orchestrator emits the
# executable plan (`elastos.drm.open.plan/v1`, status `planned`) and NEVER a
# session or key material; executing the plan is the runtime host's job.
run_case "valid open emits the canonical plan (planned, no session/key)" \
  "$VALID_OPEN" '"schema":"elastos.drm.open.plan/v1"' || failures=$((failures + 1))
plan_out="$(printf '%s\n' "$VALID_OPEN" | wasmtime run "$WASM" 2>/dev/null)"
if [[ "$plan_out" == *'"status":"planned"'* && "$plan_out" != *'"wrapped_cek"'* && "$plan_out" != *'decrypt_session_pub'* ]]; then
  echo "   PASS (plan is planned-only: no CEK, no session material)"
else
  echo "   FAIL (plan must be planned-only with no key/session material)"
  failures=$((failures + 1))
fi

run_case "non-sealed object rejected" \
  "$NON_SEALED" '"code":"invalid_request"' || failures=$((failures + 1))

echo
if [[ $failures -eq 0 ]]; then
  echo "wasm-smoke: ALL PASS (drm-provider orchestrator executes correctly + fails closed in the WASI sandbox)"
  exit 0
fi
echo "wasm-smoke: ${failures} case(s) FAILED" >&2
exit 1
