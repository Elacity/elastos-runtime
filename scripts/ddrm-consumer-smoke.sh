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
ORCH="${REPO_ROOT}/scripts/dev/ddrm-runtime-open"

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
# chain-rights: render the on-chain ownership answer into a typed receipt.
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

# Optional live wallet-ownership check: when DDRM_SMOKE_CHAIN_RPC is set we build and
# pass the REAL chain-provider so the rights step queries the AuthorityGateway on Base
# (your wallet vs the content's contentId). Offline (default) the orchestrator uses a
# deterministic mocked-owned attestation and chain-provider is never built/spawned.
#   DDRM_SMOKE_CHAIN_RPC=https://...   (required to enable live mode)
#   DDRM_SMOKE_CHAIN_CONTRACT=0x...    AuthorityGateway address
#   DDRM_SMOKE_CHAIN_SELECTOR=0x...    has_access selector
#   DDRM_SMOKE_CHAIN_SUBJECT=0x...     your wallet address
#   DDRM_SMOKE_CONTENT_ID=...          on-chain contentId/KID (defaults to golden CID)
#   DDRM_SMOKE_CHAIN_NETWORK=base  DDRM_SMOKE_CHAIN_ID=8453   (optional)
CHAIN_ARG=()
if [[ -n "${DDRM_SMOKE_CHAIN_RPC:-}" ]]; then
  build chain-provider
  CHAIN_BIN="${CAPSULES}/chain-provider/target/debug/chain-provider"
  if [[ ! -x "$CHAIN_BIN" ]]; then
    echo "FAIL: missing built binary ${CHAIN_BIN}" >&2
    exit 1
  fi
  CHAIN_ARG=("$CHAIN_BIN")
  echo "live chain mode: querying ${DDRM_SMOKE_CHAIN_RPC}"
fi

echo
echo "=============================================================="
echo " Orchestrating the chain — via the runtime-core open entrypoint"
echo "=============================================================="

# The smoke no longer assembles the host: it writes a TYPED CONFIG and INVOKES the
# default-on runtime-core entrypoint `ddrm-runtime-open` (mode=verify, which also drives the
# adversarial fail-closed gates). The runtime bin owns publish -> DrmHost::launch -> open ->
# persist. We pass the provider binaries + a per-run work dir through the config file.
CHAIN_BIN_JSON=""
if [[ ${#CHAIN_ARG[@]} -gt 0 ]]; then
  CHAIN_BIN_JSON=",\n  \"chain_bin\": \"${CHAIN_ARG[0]}\""
fi
WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ddrm-runtime-open.XXXXXX")"
CONFIG_JSON="${WORK_DIR}/open-config.json"
printf '{\n  "mode": "verify",\n  "key_bin": "%s",\n  "decrypt_bin": "%s",\n  "drm_bin": "%s",\n  "rights_bin": "%s",\n  "work_dir": "%s"%b\n}\n' \
  "$KEY_BIN" "$DECRYPT_BIN" "$DRM_BIN" "$RIGHTS_BIN" "${WORK_DIR}/run" "$CHAIN_BIN_JSON" > "$CONFIG_JSON"
echo "config: ${CONFIG_JSON}"

cargo run --quiet --manifest-path "${ORCH}/Cargo.toml" -- "$CONFIG_JSON"
status=$?
rm -rf "$WORK_DIR"

echo
if [[ $status -eq 0 ]]; then
  echo "ddrm-consumer-smoke: PASS"
  exit 0
fi
echo "ddrm-consumer-smoke: FAIL" >&2
exit 1
