#!/usr/bin/env bash
#
# dDRM producer-half orchestration smoke (Phase C, Day 60).
#
# Builds the REAL capsule binaries and drives the PRODUCER half of the Elacity dDRM
# chain end to end:
#
#     encrypt (mint CEK + seal_inline) -> key (recover-from-escrow + re-seal) -> decrypt
#
# A CEK is minted RIGHT NOW inside encrypt-provider, used to CENC-encrypt fresh
# plaintext, and ESCROWED (sealed) to the key authority's published recipient key. The
# authority recovers it from the escrow blob (never a raw CEK on the wire), re-seals it
# to the decrypt boundary's freshly-minted session key, and the boundary unwraps it
# in-VM and decrypts the segment sealed in THIS run — "a video sealed now decrypts now".
# Neither the CEK nor the plaintext ever crosses a process boundary. No golden.
#
# Usage:  scripts/ddrm-producer-smoke.sh
# Exit:   0 on PASS, 1 on FAIL.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CAPSULES="${REPO_ROOT}/capsules"
ORCH="${REPO_ROOT}/scripts/dev/ddrm-producer-smoke"

echo "=============================================================="
echo " dDRM producer-half smoke — building real capsule binaries"
echo "=============================================================="

build() {
  local pkg="$1"; shift
  echo "building ${pkg} ($*) ..."
  if ! cargo build --quiet --manifest-path "${CAPSULES}/${pkg}/Cargo.toml" "$@"; then
    echo "FAIL: could not build ${pkg}" >&2
    exit 1
  fi
}

# escrow: the producer mints + escrows a CEK to the authority's recipient key.
build encrypt-provider --features escrow
# key-authority-ref: recover-from-escrow + re-seal to the decrypt session.
build key-provider --features key-authority-ref
# rail-material + rail-mint: the canonical OpenSessionV1 + in-sandbox session mint.
build decrypt-provider --features rail-material,rail-mint

ENCRYPT_BIN="${CAPSULES}/encrypt-provider/target/debug/encrypt-provider"
KEY_BIN="${CAPSULES}/key-provider/target/debug/key-provider"
DECRYPT_BIN="${CAPSULES}/decrypt-provider/target/debug/decrypt-provider"

for bin in "$ENCRYPT_BIN" "$KEY_BIN" "$DECRYPT_BIN"; do
  if [[ ! -x "$bin" ]]; then
    echo "FAIL: missing built binary ${bin}" >&2
    exit 1
  fi
done

echo
echo "=============================================================="
echo " Orchestrating the producer half"
echo "=============================================================="

cargo run --quiet --manifest-path "${ORCH}/Cargo.toml" -- \
  "$ENCRYPT_BIN" "$KEY_BIN" "$DECRYPT_BIN"
status=$?

echo
if [[ $status -eq 0 ]]; then
  echo "ddrm-producer-smoke: PASS"
  exit 0
fi
echo "ddrm-producer-smoke: FAIL" >&2
exit 1
