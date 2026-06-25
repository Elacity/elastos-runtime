#!/usr/bin/env bash
#
# dDRM producer->chain->discovery market smoke (Phase C, Day 64).
#
# Builds the REAL capsule binaries and drives the full producer->discovery seam:
#
#     publish (prepare_publish) -> chain (assemble_mint) -> content-market (reconstruct_listing)
#
# publish-provider binds contentId == bytes16 KID; chain-provider ABI-encodes the PC2
# mint(...) calldata; content-market decodes THAT SAME calldata back into a ContentListingV1.
# The smoke proves ONE identity survives every hop — the listing's content_id equals the
# contentId publish bound equals 0x{KID} — and that discovery neither mints, signs, nor
# touches RPC/IPFS. PAID and FREE both flow. (The mint selector is configured, not computed.)
#
# Usage:  scripts/ddrm-market-smoke.sh
# Exit:   0 on PASS, 1 on FAIL.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CAPSULES="${REPO_ROOT}/capsules"
ORCH="${REPO_ROOT}/scripts/dev/ddrm-market-smoke"

echo "=============================================================="
echo " dDRM market smoke — building real capsule binaries"
echo "=============================================================="

build() {
  local pkg="$1"; shift
  echo "building ${pkg} ($*) ..."
  if ! cargo build --quiet --manifest-path "${CAPSULES}/${pkg}/Cargo.toml" "$@"; then
    echo "FAIL: could not build ${pkg}" >&2
    exit 1
  fi
}

build publish-provider
build chain-provider
build content-market

PUBLISH_BIN="${CAPSULES}/publish-provider/target/debug/publish-provider"
CHAIN_BIN="${CAPSULES}/chain-provider/target/debug/chain-provider"
MARKET_BIN="${CAPSULES}/content-market/target/debug/content-market"

for bin in "$PUBLISH_BIN" "$CHAIN_BIN" "$MARKET_BIN"; do
  if [[ ! -x "$bin" ]]; then
    echo "FAIL: missing built binary ${bin}" >&2
    exit 1
  fi
done

echo
echo "=============================================================="
echo " Orchestrating publish -> chain -> content-market"
echo "=============================================================="

cargo run --quiet --manifest-path "${ORCH}/Cargo.toml" -- \
  "$PUBLISH_BIN" "$CHAIN_BIN" "$MARKET_BIN"
status=$?

echo
if [[ $status -eq 0 ]]; then
  echo "ddrm-market-smoke: PASS"
  exit 0
fi
echo "ddrm-market-smoke: FAIL" >&2
exit 1
