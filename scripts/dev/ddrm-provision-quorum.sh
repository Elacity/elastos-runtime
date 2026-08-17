#!/usr/bin/env bash
#
# Provision a persistent 2-of-3 dKMS quorum for the Create portal (mint) + dDRM recovery.
#
# This stands up THREE long-lived `dkms-authority` node identities — each with its OWN durable
# master-seed store — and emits the artifacts the runtime needs:
#
#   <quorum-dir>/quorum.json        PUBLIC-ONLY descriptor (threshold.nodes[]: verifying + recipient
#                                   keys). The Create portal reads THIS (ELASTOS_DKMS_QUORUM_DESCRIPTOR
#                                   or <data_dir>/dkms/quorum.json) and SEALS each minted CEK share to
#                                   these recipient pubkeys. NO secret material — safe to publish.
#   <quorum-dir>/quorum-nodes.json  OPERATOR-PRIVATE sidecar (per-node store path + intended socket
#                                   endpoint). The recovery daemons read THIS to come back up with the
#                                   SAME identities the mint sealed to. Holds file paths, NOT secrets;
#                                   the master seed itself lives only inside each node's store.
#
# MINTING needs only the PUBLIC descriptor — sealing a CEK to the quorum is pure local crypto, no node
# need be running (a million sovereign minters hit the quorum ZERO times). The node DAEMONS only need to
# run later, for RECOVERY/playback (2-of-3 release). The stores are durable, so the identities the mint
# sealed to are exactly the ones the daemons serve.
#
# Usage:  scripts/dev/ddrm-provision-quorum.sh [QUORUM_DIR]
#   QUORUM_DIR  where to write the descriptor + stores. Default: the gateway's data dir
#               (<macOS app-support>/elastos/dkms), so the Create portal finds it with no env.
# Exit:   0 on success (prints the descriptor path), non-zero on failure.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CAPSULES="${REPO_ROOT}/capsules"

# Default to the gateway data dir so `quorum.json` is found at <data_dir>/dkms/quorum.json with no env.
default_data_dir() {
  case "$(uname -s)" in
    Darwin) echo "${HOME}/Library/Application Support/elastos" ;;
    *)      echo "${ELASTOS_DATA_DIR:-${HOME}/.elastos}" ;;
  esac
}

QUORUM_DIR="${1:-$(default_data_dir)/dkms}"
NODE_COUNT=3

echo "=============================================================="
echo " dDRM quorum provisioning — 2-of-3 dKMS (Create portal + recovery)"
echo "=============================================================="
echo "quorum dir: ${QUORUM_DIR}"

if ! command -v python3 >/dev/null 2>&1; then
  echo "FAIL: python3 is required to assemble the descriptor JSON" >&2
  exit 1
fi

echo "building dkms-authority ..."
if ! cargo build --quiet --manifest-path "${CAPSULES}/dkms-authority/Cargo.toml"; then
  echo "FAIL: could not build dkms-authority" >&2
  exit 1
fi
NODE_BIN="${CAPSULES}/dkms-authority/target/debug/dkms-authority"
if [[ ! -x "$NODE_BIN" ]]; then
  echo "FAIL: missing built binary ${NODE_BIN}" >&2
  exit 1
fi

mkdir -p "${QUORUM_DIR}"
chmod 700 "${QUORUM_DIR}" 2>/dev/null || true

# Provision one node OFFLINE: ensure a durable store and read back the node's PUBLIC identity
# (verifying key + escrow recipient) via the `provision` subcommand. DKMS-7: identity creation is
# OFFLINE from the operator-owned state root — never a wire `init`, so a client cannot select the
# key-store path or trigger first creation. First run CREATES the master seed; later runs are
# deterministic from it (idempotent — re-running keeps the same identities the mint already sealed
# to). Echoes `vk<TAB>recipient<TAB>store<TAB>endpoint`.
provision_node() {
  local idx="$1"
  local store="${QUORUM_DIR}/node-${idx}.store.json"
  local endpoint="${QUORUM_DIR}/node-${idx}.sock"
  local resp
  resp="$(DKMS_AUTHORITY_KEY_STORE="$store" "$NODE_BIN" provision 2>/dev/null | head -n 1)"
  if [[ -z "$resp" ]]; then
    echo "FAIL: node ${idx} published no identity on provision" >&2
    return 1
  fi
  python3 - "$resp" "$store" "$endpoint" <<'PY'
import json, sys
resp, store, endpoint = sys.argv[1], sys.argv[2], sys.argv[3]
try:
    obj = json.loads(resp)
except Exception as e:
    sys.stderr.write(f"node provision response is not JSON: {e}\n"); sys.exit(1)
vk = obj.get("seal_verifying_key_b64")
rec = obj.get("seal_recipient_pub_b64")
if not vk or not rec:
    sys.stderr.write(f"node provision missing public identity: {resp}\n"); sys.exit(1)
print("\t".join([vk, rec, store, endpoint]))
PY
}

VKS=(); RECS=(); STORES=(); ENDPOINTS=()
for i in $(seq 0 $((NODE_COUNT - 1))); do
  echo "provisioning node ${i} ..."
  line="$(provision_node "$i")" || exit 1
  IFS=$'\t' read -r vk rec store endpoint <<<"$line"
  VKS+=("$vk"); RECS+=("$rec"); STORES+=("$store"); ENDPOINTS+=("$endpoint")
done

# Distinct identities are a hard requirement (three independent secret-holders, not one masquerading).
if [[ "${VKS[0]}" == "${VKS[1]}" || "${VKS[1]}" == "${VKS[2]}" || "${VKS[0]}" == "${VKS[2]}" ]]; then
  echo "FAIL: provisioned nodes did not produce three DISTINCT identities" >&2
  exit 1
fi

PUBLIC_DESCRIPTOR="${QUORUM_DIR}/quorum.json"
PRIVATE_SIDECAR="${QUORUM_DIR}/quorum-nodes.json"

# Assemble both files. The PUBLIC descriptor carries ONLY verifying + recipient keys (what the Create
# portal seals to); the PRIVATE sidecar carries the operator's store paths + endpoints (what the
# recovery daemons come back up with). Neither carries a master seed.
python3 - "$PUBLIC_DESCRIPTOR" "$PRIVATE_SIDECAR" "${VKS[@]}" "__SEP__" "${RECS[@]}" "__SEP__" "${STORES[@]}" "__SEP__" "${ENDPOINTS[@]}" <<'PY'
import json, sys
args = sys.argv[1:]
public_path, private_path = args[0], args[1]
rest = args[2:]
groups, cur = [], []
for a in rest:
    if a == "__SEP__":
        groups.append(cur); cur = []
    else:
        cur.append(a)
groups.append(cur)
vks, recs, stores, endpoints = groups
nodes_public = [{"verifying_key_b64": v, "recipient_pub_b64": r} for v, r in zip(vks, recs)]
public = {
    "schema": "elastos.dkms.quorum_descriptor/v1",
    "threshold": {"t": 2, "n": len(nodes_public), "nodes": nodes_public},
}
private = {
    "schema": "elastos.dkms.quorum_nodes/v1",
    "note": "OPERATOR-PRIVATE: store paths + endpoints for the recovery daemons. Not the public descriptor.",
    "nodes": [
        {"verifying_key_b64": v, "recipient_pub_b64": r, "key_store": s, "authority_endpoint": e}
        for v, r, s, e in zip(vks, recs, stores, endpoints)
    ],
}
with open(public_path, "w") as f:
    json.dump(public, f, indent=2); f.write("\n")
with open(private_path, "w") as f:
    json.dump(private, f, indent=2); f.write("\n")
print(f"wrote {public_path}")
print(f"wrote {private_path}")
PY
status=$?
if [[ $status -ne 0 ]]; then
  echo "FAIL: could not write the quorum descriptor" >&2
  exit 1
fi
chmod 600 "${PRIVATE_SIDECAR}" 2>/dev/null || true
for s in "${STORES[@]}"; do chmod 600 "$s" 2>/dev/null || true; done

echo
echo "=============================================================="
echo " quorum provisioned — 2-of-3, three distinct secret-holders"
echo "=============================================================="
echo "PUBLIC descriptor (Create portal reads this): ${PUBLIC_DESCRIPTOR}"
echo "  → the gateway finds it automatically at <data_dir>/dkms/quorum.json,"
echo "    or point at it with ELASTOS_DKMS_QUORUM_DESCRIPTOR=${PUBLIC_DESCRIPTOR}"
echo "PRIVATE sidecar (recovery daemons read this): ${PRIVATE_SIDECAR}"
echo
echo "Minting is ready now (sealing to the quorum is pure local crypto — no node need run)."
echo "The node DAEMONS only need to run later, for playback/recovery."
