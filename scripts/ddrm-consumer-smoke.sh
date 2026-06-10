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
# Usage:  scripts/ddrm-consumer-smoke.sh [--backend reference|dkms]
#   --backend reference  (default) the in-runtime durable-key-store authority
#   --backend dkms       the EXTERNAL, SECRET-HOLDING authority NODE (dkms-authority capsule):
#                        the publish phase PROVISIONS the node (its master stays in the node's own
#                        store) + writes a PUBLIC-ONLY descriptor (pins + endpoint, NO secret); at
#                        open, the key-provider holds only that public identity and DELEGATES
#                        recovery to the node — the master/CEK NEVER enter the runtime. The open
#                        path is otherwise byte-identical (only OpenConfig.authority differs),
#                        like PC2's getSessionView dispatch + recoverCEKEnvelope delegation.
# Exit:   0 on PASS, 1 on FAIL.

set -uo pipefail

BACKEND="reference"
THRESHOLD="false"
TRANSPORT="unix"
NODES="2"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --backend) BACKEND="${2:-}"; shift 2 ;;
    --backend=*) BACKEND="${1#*=}"; shift ;;
    # 2-of-2 THRESHOLD (Day 99–100): the runtime provisions TWO secret-holding nodes, XOR-splits the
    # CEK so neither node ever holds the whole key, and drives the full dual-recover + in-VM combine.
    # Implies --backend dkms (the only backend with external secret-holders).
    --threshold) THRESHOLD="true"; BACKEND="dkms"; shift ;;
    # 2-of-3 QUORUM (Day 113–116): THREE secret-holding nodes, the CEK Shamir-split over GF(256)
    # into indexed shares — ANY TWO live nodes serve an open (the rail survives a dead node), and
    # below quorum it fails closed. Implies --threshold (and so --backend dkms).
    --nodes) NODES="${2:-}"; shift 2 ;;
    --nodes=*) NODES="${1#*=}"; shift ;;
    # NETWORK TRANSPORT (Day 105–108): the dKMS node daemons listen on REAL TCP endpoints
    # (tcp:127.0.0.1:PORT) instead of Unix sockets; the rail requires the app-layer encrypted,
    # mutually-authenticated channel and the network adversarial gates run (plaintext recover
    # refused, downgrade dropped, MITM-tampered frame dropped, wrong channel key refused).
    # Implies --backend dkms (it addresses the external node daemons).
    --transport) TRANSPORT="${2:-}"; shift 2 ;;
    --transport=*) TRANSPORT="${1#*=}"; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [[ "$BACKEND" != "reference" && "$BACKEND" != "dkms" ]]; then
  echo "FAIL: --backend must be reference|dkms (got '${BACKEND}')" >&2
  exit 2
fi
if [[ "$THRESHOLD" == "true" && "$BACKEND" != "dkms" ]]; then
  echo "FAIL: --threshold requires --backend dkms" >&2
  exit 2
fi
if [[ "$TRANSPORT" != "unix" && "$TRANSPORT" != "tcp" ]]; then
  echo "FAIL: --transport must be unix|tcp (got '${TRANSPORT}')" >&2
  exit 2
fi
if [[ "$TRANSPORT" == "tcp" && "$BACKEND" != "dkms" ]]; then
  echo "FAIL: --transport tcp requires --backend dkms" >&2
  exit 2
fi
if [[ "$NODES" != "2" && "$NODES" != "3" ]]; then
  echo "FAIL: --nodes must be 2|3 (got '${NODES}')" >&2
  exit 2
fi
if [[ "$NODES" == "3" && "$THRESHOLD" != "true" ]]; then
  echo "FAIL: --nodes 3 requires --threshold" >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CAPSULES="${REPO_ROOT}/capsules"
ORCH="${REPO_ROOT}/scripts/dev/ddrm-runtime-open"

MODE_LABEL="${BACKEND}"
if [[ "$THRESHOLD" == "true" ]]; then
  if [[ "$NODES" == "3" ]]; then
    MODE_LABEL="${BACKEND}, 2-of-3 quorum"
  else
    MODE_LABEL="${BACKEND}, 2-of-2 threshold"
  fi
fi
if [[ "$TRANSPORT" == "tcp" ]]; then MODE_LABEL="${MODE_LABEL}, tcp + encrypted channel"; fi

echo "=============================================================="
echo " dDRM consumer-half smoke (authority=${MODE_LABEL}) — building real capsule binaries"
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

# The `dkms` backend needs the EXTERNAL authority NODE binary (the secret-holding capsule the
# publish phase provisions + the open delegates recovery to). Built only for that backend.
DKMS_NODE_BIN=""
if [[ "$BACKEND" == "dkms" ]]; then
  build dkms-authority
  DKMS_NODE_BIN="${CAPSULES}/dkms-authority/target/debug/dkms-authority"
  if [[ ! -x "$DKMS_NODE_BIN" ]]; then
    echo "FAIL: missing built binary ${DKMS_NODE_BIN}" >&2
    exit 1
  fi
fi

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
# persist. We pass the provider binaries + a per-run work dir + the selected authority backend
# through the config file. Switching backends changes ONLY `authority.backend` — same binary,
# same flow — proving the open is backend-agnostic.
CHAIN_BIN_JSON=""
if [[ ${#CHAIN_ARG[@]} -gt 0 ]]; then
  CHAIN_BIN_JSON=",\n  \"chain_bin\": \"${CHAIN_ARG[0]}\""
fi
# For `dkms`, the authority object ALSO names the external node binary the runtime provisions +
# delegates recovery to (its master never crosses into the runtime).
AUTHORITY_JSON="{ \"backend\": \"${BACKEND}\" }"
if [[ "$BACKEND" == "dkms" ]]; then
  AUTHORITY_JSON="{ \"backend\": \"dkms\", \"dkms_authority_bin\": \"${DKMS_NODE_BIN}\", \"threshold\": ${THRESHOLD}, \"nodes\": ${NODES}, \"transport\": \"${TRANSPORT}\" }"
fi
WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ddrm-runtime-open.XXXXXX")"
CONFIG_JSON="${WORK_DIR}/open-config.json"
printf '{\n  "mode": "verify",\n  "authority": %s,\n  "key_bin": "%s",\n  "decrypt_bin": "%s",\n  "drm_bin": "%s",\n  "rights_bin": "%s",\n  "work_dir": "%s"%b\n}\n' \
  "$AUTHORITY_JSON" "$KEY_BIN" "$DECRYPT_BIN" "$DRM_BIN" "$RIGHTS_BIN" "${WORK_DIR}/run" "$CHAIN_BIN_JSON" > "$CONFIG_JSON"
echo "config: ${CONFIG_JSON}"

cargo run --quiet --manifest-path "${ORCH}/Cargo.toml" -- "$CONFIG_JSON"
status=$?
rm -rf "$WORK_DIR"

echo
if [[ $status -eq 0 ]]; then
  echo "ddrm-consumer-smoke (authority=${MODE_LABEL}): PASS"
  exit 0
fi
echo "ddrm-consumer-smoke (authority=${MODE_LABEL}): FAIL" >&2
exit 1
