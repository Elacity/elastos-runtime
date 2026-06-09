#!/usr/bin/env bash
#
# dDRM consumer-half orchestration smoke (Phase A.4).
#
# Builds the REAL capsule binaries and drives them through the consumer half of the
# Elacity dDRM chain end to end:
#
#     drm/open -> rights -> key (reference authority) -> decrypt (OpenSessionV1)
#
# proving the cross-process key->decrypt handoff works with NO Lit, NO dKMS and NO
# chain: the authority seals a CEK to the decrypt boundary's freshly-minted, published
# session key (transcript-bound via the SHARED ddrm-envelope encoder), the boundary
# unwraps it in-VM and decrypts a real CENC segment, and neither the CEK nor the
# plaintext ever crosses a process boundary. A transcript-mismatched seal fails closed.
#
# This is the first point a human can drive the consumer half and SEE it run.
#
# Usage:  scripts/ddrm-consumer-smoke.sh
# Exit:   0 on PASS, 1 on FAIL.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CAPSULES="${REPO_ROOT}/capsules"
ORCH="${REPO_ROOT}/scripts/dev/ddrm-consumer-smoke"

echo "=============================================================="
echo " dDRM consumer-half smoke — building real capsule binaries"
echo "=============================================================="

build() {
  local pkg="$1"; shift
  echo "building ${pkg} ($*) ..."
  if ! cargo build --quiet --manifest-path "${CAPSULES}/${pkg}/Cargo.toml" "$@"; then
    echo "FAIL: could not build ${pkg}" >&2
    exit 1
  fi
}

build key-provider --features key-authority-ref
# rail-material gives the canonical OpenSessionV1; rail-mint gives the in-sandbox
# session mint + publish at init. The consumer half needs both.
build decrypt-provider --features rail-material,rail-mint
build drm-provider
# chain-rights: render the (mocked) on-chain ownership answer into a typed receipt.
build rights-provider --features chain-rights

KEY_BIN="${CAPSULES}/key-provider/target/debug/key-provider"
DECRYPT_BIN="${CAPSULES}/decrypt-provider/target/debug/decrypt-provider"
DRM_BIN="${CAPSULES}/drm-provider/target/debug/drm-provider"
RIGHTS_BIN="${CAPSULES}/rights-provider/target/debug/rights-provider"

for bin in "$KEY_BIN" "$DECRYPT_BIN" "$DRM_BIN" "$RIGHTS_BIN"; do
  if [[ ! -x "$bin" ]]; then
    echo "FAIL: missing built binary ${bin}" >&2
    exit 1
  fi
done

echo
echo "=============================================================="
echo " Orchestrating the chain"
echo "=============================================================="

cargo run --quiet --manifest-path "${ORCH}/Cargo.toml" -- \
  "$KEY_BIN" "$DECRYPT_BIN" "$DRM_BIN" "$RIGHTS_BIN"
status=$?

echo
if [[ $status -eq 0 ]]; then
  echo "ddrm-consumer-smoke: PASS"
  exit 0
fi
echo "ddrm-consumer-smoke: FAIL" >&2
exit 1
