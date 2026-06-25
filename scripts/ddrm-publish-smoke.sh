#!/usr/bin/env bash
#
# dDRM producer->chain publish smoke (Phase C, Day 63).
#
# Builds the REAL capsule binaries and drives the producer->chain seam end to end:
#
#     publish-provider (prepare_publish)  ->  chain-provider (assemble_mint)
#
# publish-provider binds contentId == bytes16 KID, derives the tokenURI, and emits a typed
# UnsignedMintV1 with STRUCTURED op/sell terms. That blob drops STRAIGHT into
# chain-provider::assemble_mint, which ABI-encodes the PC2 mint(string,uint16,bytes,bytes)
# calldata. The smoke proves ONE identity flows KID -> contentId -> mint calldata across
# both binaries with the tokenURI + sell terms intact — and that the assembler neither
# signs nor touches RPC. (The mint selector is configured, not computed in-capsule.)
#
# Usage:  scripts/ddrm-publish-smoke.sh
# Exit:   0 on PASS, 1 on FAIL.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CAPSULES="${REPO_ROOT}/capsules"
ORCH="${REPO_ROOT}/scripts/dev/ddrm-publish-smoke"

echo "=============================================================="
echo " dDRM publish smoke — building real capsule binaries"
echo "=============================================================="

build() {
  local pkg="$1"; shift
  echo "building ${pkg} ($*) ..."
  if ! cargo build --quiet --manifest-path "${CAPSULES}/${pkg}/Cargo.toml" "$@"; then
    echo "FAIL: could not build ${pkg}" >&2
    exit 1
  fi
}

# Both default-featured: publish assembles the unsigned mint, chain ABI-encodes it.
build publish-provider
build chain-provider

PUBLISH_BIN="${CAPSULES}/publish-provider/target/debug/publish-provider"
CHAIN_BIN="${CAPSULES}/chain-provider/target/debug/chain-provider"

for bin in "$PUBLISH_BIN" "$CHAIN_BIN"; do
  if [[ ! -x "$bin" ]]; then
    echo "FAIL: missing built binary ${bin}" >&2
    exit 1
  fi
done

echo
echo "=============================================================="
echo " Orchestrating publish -> chain"
echo "=============================================================="

cargo run --quiet --manifest-path "${ORCH}/Cargo.toml" -- \
  "$PUBLISH_BIN" "$CHAIN_BIN"
status=$?

echo
if [[ $status -eq 0 ]]; then
  echo "ddrm-publish-smoke: PASS"
  exit 0
fi
echo "ddrm-publish-smoke: FAIL" >&2
exit 1
