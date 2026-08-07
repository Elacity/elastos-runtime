#!/usr/bin/env bash
#
# Unified dDRM chain smoke — runs the WASI-sandbox smoke for every provider in the
# Elacity dDRM chain (drm/open -> rights -> key -> decrypt) and emits one
# consolidated PASS/FAIL report.
#
# This is the "the whole chain is fail-closed and wasm-proven" demo: a single
# command that builds each provider for wasm32-wasip1 (if needed) and executes it
# under wasmtime, asserting the fail-closed security contract holds end-to-end.
#
# Prerequisites: rustup target add wasm32-wasip1 ; brew install wasmtime
# Usage:  scripts/ddrm-chain-smoke.sh
# Exit:   0 if every provider passes, 1 otherwise.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CAPSULES_DIR="${REPO_ROOT}/capsules"

# Chain order: front door first, then the providers it sequences.
PROVIDERS=(
  "drm-provider"
  "rights-provider"
  "key-provider"
  "decrypt-provider"
)

if ! command -v wasmtime >/dev/null 2>&1; then
  echo "FAIL: wasmtime CLI not found (install: brew install wasmtime)" >&2
  exit 1
fi

echo "=============================================================="
echo " Elacity dDRM chain — WASI-sandbox smoke (drm -> rights -> key -> decrypt)"
echo "=============================================================="

failures=0
declare -a results

for provider in "${PROVIDERS[@]}"; do
  smoke="${CAPSULES_DIR}/${provider}/scripts/wasm-smoke.sh"
  echo
  echo "##############################################################"
  echo "# ${provider}"
  echo "##############################################################"
  if [[ ! -x "$smoke" ]]; then
    echo "FAIL: missing smoke harness at ${smoke}"
    results+=("FAIL  ${provider} (no harness)")
    failures=$((failures + 1))
    continue
  fi
  if "$smoke"; then
    results+=("PASS  ${provider}")
  else
    results+=("FAIL  ${provider}")
    failures=$((failures + 1))
  fi
done

echo
echo "=============================================================="
echo " dDRM chain summary"
echo "=============================================================="
for line in "${results[@]}"; do
  echo "  ${line}"
done
echo

if [[ $failures -eq 0 ]]; then
  echo "dDRM chain: ALL PROVIDERS PASS — fail-closed + wasm/WASI-proven end-to-end."
  echo "(Live decrypt remains gated on the CEK/ciphertext rail; see docs/dkms/history/DDRM_DECRYPT_RAIL.md)"
  exit 0
fi
echo "dDRM chain: ${failures} provider(s) FAILED" >&2
exit 1
