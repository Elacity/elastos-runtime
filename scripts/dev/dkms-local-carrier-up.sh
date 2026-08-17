#!/usr/bin/env bash
#
# Stand up a LOCAL dKMS quorum behind Carrier bridges and emit the carrier descriptor
# that `run-creator-gateway.sh --carrier` consumes (~/.elastos-dkms/dkms-authority.carrier.json).
#
# This is the local mirror of the live geo-node wiring (docs/DKMS_OVER_CARRIER.md):
#
#   key-provider -> dkms-carrier-client -> Carrier/iroh -> dkms-carrier-node -> dkms-authority(tcp)
#
# It reuses EXISTING durable node stores, so the quorum identities are exactly the ones
# already sealed to — only the transport front changes. The Carrier bridge dials TCP, so the
# daemons are started on loopback TCP listeners (identity lives in the store, not the socket).
#
# Usage:
#   scripts/dev/dkms-local-carrier-up.sh up   [QUORUM_SRC_DIR]   # default: the dkms quorum
#   scripts/dev/dkms-local-carrier-up.sh down
#
#   QUORUM_SRC_DIR must contain node-{0,1,2}.store.json + quorum.json (PQ pins).
#
# Artifacts (all under ~/.elastos-dkms):
#   dkms-authority.carrier.json   the v2 carrier descriptor (what --carrier reads)
#   bridge-{0,1,2}.seed           persisted iroh identities -> STABLE did:key across restarts
#   logs/                         daemon + bridge logs
#   carrier-local.pids            pids for `down`
#
# Env honored on `up`:
#   ELASTOS_DDRM_RIGHTS=chain   -> pass the node-side trustless Base authz env to each daemon
#                                  (same defaults as run-creator-gateway.sh)
#
# Then launch the gateway in another terminal:
#   ELASTOS_DDRM_RIGHTS=chain ./scripts/dev/run-creator-gateway.sh --carrier

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CAPSULES="${REPO_ROOT}/capsules"
OUT_DIR="${HOME}/.elastos-dkms"
LOG_DIR="${OUT_DIR}/logs"
PID_FILE="${OUT_DIR}/carrier-local.pids"
DESCRIPTOR="${OUT_DIR}/dkms-authority.carrier.json"
CALLER_SEED="${OUT_DIR}/secrets/caller.seed"
PORTS=(19471 19472 19473)

CMD="${1:-up}"

if [[ "$CMD" == "down" ]]; then
  if [[ -f "$PID_FILE" ]]; then
    while IFS= read -r p; do [[ -n "$p" ]] && kill "$p" 2>/dev/null; done < "$PID_FILE"
    rm -f "$PID_FILE"
    echo "stopped local carrier quorum (daemons + bridges)"
  else
    echo "nothing to stop (${PID_FILE} not found)"
  fi
  exit 0
fi
[[ "$CMD" == "up" ]] || { echo "usage: $0 up [QUORUM_SRC_DIR] | down" >&2; exit 2; }

default_src() {
  case "$(uname -s)" in
    Darwin) echo "${HOME}/Library/Application Support/elastos/dkms" ;;
    *)      echo "${ELASTOS_DATA_DIR:-${HOME}/.elastos}/dkms" ;;
  esac
}
SRC_DIR="${2:-$(default_src)}"

for i in 0 1 2; do
  [[ -f "${SRC_DIR}/node-${i}.store.json" ]] || { echo "FAIL: missing ${SRC_DIR}/node-${i}.store.json" >&2; exit 1; }
done
[[ -f "${SRC_DIR}/quorum.json" ]] || { echo "FAIL: missing ${SRC_DIR}/quorum.json (PQ pins)" >&2; exit 1; }
[[ -f "$CALLER_SEED" ]] || { echo "FAIL: missing caller seed ${CALLER_SEED} (the --carrier gateway reads it)" >&2; exit 1; }

if [[ -f "$PID_FILE" ]]; then
  echo "FAIL: ${PID_FILE} exists — a local carrier quorum may already be running. Run: $0 down" >&2
  exit 1
fi

echo "quorum source: ${SRC_DIR}"
echo "descriptor out: ${DESCRIPTOR}"
mkdir -p "$LOG_DIR"

echo "building dkms-authority, dkms-keygen, dkms-carrier-node ..."
build() {
  cargo build --quiet --manifest-path "$1/Cargo.toml" || { echo "FAIL build $1" >&2; exit 1; }
}
build "${CAPSULES}/dkms-authority"
build "${CAPSULES}/dkms-keygen"
build "${REPO_ROOT}/scripts/dev/dkms-carrier-node"
NODE_BIN="${CAPSULES}/dkms-authority/target/debug/dkms-authority"
KEYGEN_BIN="${CAPSULES}/dkms-keygen/target/debug/dkms-keygen"
BRIDGE_BIN="${REPO_ROOT}/scripts/dev/dkms-carrier-node/target/debug/dkms-carrier-node"

# The nodes allow-list the SAME caller identity the --carrier gateway authenticates with.
CALLER_SEED_B64="$(tr -d '\r\n' < "$CALLER_SEED")"
CALLER_VK="$("$KEYGEN_BIN" derive-vk --seed-b64 "$CALLER_SEED_B64" | tr -d '\n')"
[[ -n "$CALLER_VK" ]] || { echo "FAIL: could not derive caller vk from ${CALLER_SEED}" >&2; exit 1; }

# Node-side trustless Base authz, same wiring as run-creator-gateway.sh (chain mode only).
NODE_CHAIN_ENV=()
if [[ "${ELASTOS_DDRM_RIGHTS:-dev}" == "chain" ]]; then
  NODE_CHAIN_ENV=(
    "DKMS_CHAIN_RPC_POOL=${DKMS_CHAIN_RPC_POOL:-${ELASTOS_CHAIN_BASE_RPC:-https://mainnet.base.org}}"
    "DKMS_RIGHTS_CONTRACT=${ELASTOS_DDRM_RIGHTS_CONTRACT:-0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D}"
    "DKMS_RIGHTS_SELECTOR=${ELASTOS_DDRM_RIGHTS_SELECTOR:-0x54d42821}"
    "DKMS_CHAIN_ID=${ELASTOS_DDRM_CHAIN_ID:-8453}"
  )
  echo "node-side trustless authz ENABLED (chain mode)"
fi

echo "starting 3 dkms-authority daemons on loopback tcp (stores: ${SRC_DIR}) ..."
: > "$PID_FILE"
for i in 0 1 2; do
  P="${PORTS[$i]}"
  if [[ "${#NODE_CHAIN_ENV[@]}" -gt 0 ]]; then
    env "${NODE_CHAIN_ENV[@]}" \
    DKMS_AUTHORITY_LISTEN="tcp:127.0.0.1:${P}" \
    DKMS_AUTHORITY_KEY_STORE="${SRC_DIR}/node-${i}.store.json" \
    DKMS_AUTHORITY_ALLOWED_CALLERS="$CALLER_VK" \
      "$NODE_BIN" >/dev/null 2>>"${LOG_DIR}/node-${i}.log" &
  else
    DKMS_AUTHORITY_LISTEN="tcp:127.0.0.1:${P}" \
    DKMS_AUTHORITY_KEY_STORE="${SRC_DIR}/node-${i}.store.json" \
    DKMS_AUTHORITY_ALLOWED_CALLERS="$CALLER_VK" \
      "$NODE_BIN" >/dev/null 2>>"${LOG_DIR}/node-${i}.log" &
  fi
  echo "$!" >> "$PID_FILE"
done
for P in "${PORTS[@]}"; do
  for _ in $(seq 1 200); do
    (exec 3<>"/dev/tcp/127.0.0.1/$P") 2>/dev/null && { exec 3>&-; break; }
    sleep 0.05
  done
done

echo "starting 3 dkms-carrier-node bridges (persistent did:key identities) ..."
DIDS=()
for i in 0 1 2; do
  P="${PORTS[$i]}"
  : > "${LOG_DIR}/bridge-${i}.out"
  DKMS_CARRIER_NODE_TARGET="127.0.0.1:${P}" \
  DKMS_CARRIER_NODE_SEED="${OUT_DIR}/bridge-${i}.seed" \
    "$BRIDGE_BIN" >"${LOG_DIR}/bridge-${i}.out" 2>"${LOG_DIR}/bridge-${i}.err" &
  echo "$!" >> "$PID_FILE"
done
for i in 0 1 2; do
  DID=""
  for _ in $(seq 1 60); do
    DID="$(head -n1 "${LOG_DIR}/bridge-${i}.out" 2>/dev/null)"
    [[ -n "$DID" ]] && break
    sleep 0.5
  done
  if [[ -z "$DID" ]]; then
    echo "FAIL: bridge ${i} produced no did:key" >&2
    tail -5 "${LOG_DIR}/bridge-${i}.err" 2>/dev/null
    bash "$0" down
    exit 1
  fi
  DIDS+=("$DID")
  echo "  node${i} did: ${DID}"
done

# v2 descriptor: PQ pins from the source quorum (unchanged), endpoints rewritten to carrier:did.
python3 - "${SRC_DIR}/quorum.json" "$DESCRIPTOR" "${DIDS[@]}" <<'PY'
import json, sys
src, out = sys.argv[1], sys.argv[2]
dids = sys.argv[3:]
q = json.load(open(src))
pins = q["threshold"]["nodes"]
assert len(pins) == len(dids), f"{len(pins)} pins vs {len(dids)} dids"
nodes = [
    {"verifying_key_b64": p["verifying_key_b64"],
     "recipient_pub_b64": p["recipient_pub_b64"],
     "authority_endpoint": f"carrier:{d}"}
    for p, d in zip(pins, dids)
]
desc = {
    "schema": "elastos.dkms.authority/v2",
    "verifying_key_b64": nodes[0]["verifying_key_b64"],
    "recipient_pub_b64": nodes[0]["recipient_pub_b64"],
    "authority_endpoint": nodes[0]["authority_endpoint"],
    "threshold": {"t": q["threshold"].get("t", 2), "n": len(nodes), "nodes": nodes},
}
json.dump(desc, open(out, "w"), indent=2)
print(f"wrote {out}")
PY
[[ -f "$DESCRIPTOR" ]] || { echo "FAIL: descriptor not written" >&2; bash "$0" down; exit 1; }
chmod 600 "$DESCRIPTOR" 2>/dev/null || true

echo
echo "=============================================================="
echo " local Carrier quorum LIVE — 3 daemons + 3 bridges"
echo "=============================================================="
echo "descriptor: ${DESCRIPTOR}"
echo "stores:     ${SRC_DIR}/node-{0,1,2}.store.json (identities unchanged)"
echo "stop with:  $0 down"
echo
echo "launch the gateway in another terminal:"
echo "  ELASTOS_DDRM_RIGHTS=chain ./scripts/dev/run-creator-gateway.sh --carrier"
