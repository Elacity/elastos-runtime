#!/usr/bin/env bash
# One-command local dKMS quorum simulation: three isolated container "servers", each
# running dkms-authority + a dkms-carrier-node bridge, then a Carrier descriptor
# assembled from their PUBLISHED identities — the same shape as the live geo quorum's
# ~/.elastos-dkms/dkms-authority.carrier.json.
#
#   ./up.sh          build + start + mint caller identity + assemble descriptor
#   ./up.sh down     stop the quorum (keeps volumes: master seeds survive restarts)
#   ./up.sh destroy  stop AND delete volumes (a brand-new quorum next time)
#
# Outputs (in ./shared):
#   dkms-authority.carrier.json   PUBLIC descriptor -> ELASTOS_DKMS_REMOTE_DESCRIPTOR
#   caller.seed                   SECRET caller identity -> ELASTOS_DKMS_REMOTE_CALLER_SEED
set -euo pipefail
cd "$(dirname "$0")"

COMPOSE="docker compose"
command -v docker >/dev/null || { echo "FAIL: docker is required" >&2; exit 1; }

case "${1:-up}" in
  down)    exec $COMPOSE down ;;
  destroy) exec $COMPOSE down -v ;;
  up)      ;;
  *) echo "usage: $0 [up|down|destroy]" >&2; exit 2 ;;
esac

mkdir -p shared
# Host-visible node logs (see docker-compose.yml x-node-logs): pre-create the bind source so
# docker never has to (root-owned) create it, and the logs sit next to the runtime's own
# dkms state under ~/Library/Application Support/ElastOS.
mkdir -p "${DKMS_NODE_LOG_DIR:-${HOME}/Library/Application Support/ElastOS/dkms/docker-logs}"

echo "== build the node image (dkms-authority + dkms-carrier-node + dkms-keygen) =="
$COMPOSE build

echo "== mint the runtime CALLER identity (once; reused across restarts) =="
if [[ ! -f shared/caller.seed || ! -f shared/caller.vk ]]; then
  $COMPOSE run --rm --no-deps --entrypoint dkms-keygen dkms-node0 \
    keygen --role caller --out /shared >/dev/null
fi
CALLER_VK="$(tr -d '\n' < shared/caller.vk)"
[[ -n "$CALLER_VK" ]] || { echo "FAIL: no caller VK minted" >&2; exit 1; }
# Preserve a previously-computed node-set pin across the .env rewrite so the containers
# start with it immediately; it is recomputed from the live descriptor further down and
# the nodes are recreated only if it actually changed.
PREV_NODE_SET_ID="$(grep -s '^DKMS_AUTHORITY_NODE_SET_ID_B64=' .env | head -n1 | cut -d= -f2- || true)"
{
  echo "DKMS_AUTHORITY_ALLOWED_CALLERS=${CALLER_VK}"
  if [[ -n "$PREV_NODE_SET_ID" ]]; then
    echo "DKMS_AUTHORITY_NODE_SET_ID_B64=${PREV_NODE_SET_ID}"
  fi
} > .env
echo "  caller VK allow-listed on all 3 nodes (seed: shared/caller.seed — SECRET)"

echo "== start the 3-node quorum =="
$COMPOSE up -d

echo "== wait for each node's PUBLIC identity + carrier did =="
for i in 0 1 2; do
  for _ in $(seq 1 240); do
    [[ -s "shared/node-${i}.init.json" && -s "shared/node-${i}.did" ]] && break
    sleep 0.5
  done
  [[ -s "shared/node-${i}.init.json" && -s "shared/node-${i}.did" ]] \
    || { echo "FAIL: node ${i} did not publish identity/did — see: $COMPOSE logs dkms-node${i}" >&2; exit 1; }
  echo "  node${i}: $(head -n1 "shared/node-${i}.did")"
done

echo "== assemble the CARRIER v2 descriptor (public pins + carrier:did endpoints) =="
python3 - <<'PY'
import json

nodes = []
for i in range(3):
    init = json.load(open(f"shared/node-{i}.init.json"))
    # DKMS-7: identity now comes from the offline `provision` subcommand, which prints the
    # seal keys at the TOP LEVEL; the legacy wire-`init` response nested them under "data".
    # Accept both so the descriptor assembles from either producer.
    data = init.get("data", init)
    vk, rec = data.get("seal_verifying_key_b64"), data.get("seal_recipient_pub_b64")
    assert vk and rec, f"node {i} provision output missing public identity"
    did = open(f"shared/node-{i}.did").readline().strip()
    assert did.startswith("did:key:"), f"node {i} did looks wrong: {did!r}"
    nodes.append({
        "verifying_key_b64": vk,
        "recipient_pub_b64": rec,
        "authority_endpoint": f"carrier:{did}",
    })

assert len({n["verifying_key_b64"] for n in nodes}) == 3, "nodes are not three DISTINCT identities"

desc = {
    "schema": "elastos.dkms.authority/v2",
    "verifying_key_b64": nodes[0]["verifying_key_b64"],
    "recipient_pub_b64": nodes[0]["recipient_pub_b64"],
    "authority_endpoint": nodes[0]["authority_endpoint"],
    "threshold": {"t": 2, "nodes": nodes},
}
with open("shared/dkms-authority.carrier.json", "w") as f:
    json.dump(desc, f, indent=2)
print("  wrote shared/dkms-authority.carrier.json (2-of-3, carrier endpoints)")
PY

echo "== pin the quorum node-set id on all 3 nodes (release-node cross-quorum replay defense) =="
# The release-built node REFUSES a grant-authorized recover unless the operator pins its
# own quorum identity (DKMS_AUTHORITY_NODE_SET_ID_B64) — it must never trust the grant's
# caller-declared node-set. Same encoding as ddrm_envelope::threshold_node_set_id_n:
# SHA-256("elastos.dkms.threshold.node-set/v1" ‖ t ‖ (u32-BE len ‖ vk) per node, in
# descriptor order), standard base64.
NODE_SET_ID_B64="$(python3 - <<'PY'
import base64, hashlib, json
d = json.load(open("shared/dkms-authority.carrier.json"))
th = d["threshold"]
h = hashlib.sha256()
h.update(b"elastos.dkms.threshold.node-set/v1")
h.update(bytes([th["t"]]))
for n in th["nodes"]:
    vk = base64.b64decode(n["verifying_key_b64"])
    h.update(len(vk).to_bytes(4, "big"))
    h.update(vk)
print(base64.b64encode(h.digest()).decode())
PY
)"
[[ -n "$NODE_SET_ID_B64" ]] || { echo "FAIL: could not compute the node-set id" >&2; exit 1; }
{
  echo "DKMS_AUTHORITY_ALLOWED_CALLERS=${CALLER_VK}"
  echo "DKMS_AUTHORITY_NODE_SET_ID_B64=${NODE_SET_ID_B64}"
} > .env
if [[ "$NODE_SET_ID_B64" != "$PREV_NODE_SET_ID" ]]; then
  echo "  pin (re)computed — recreating the nodes so they enforce it"
  $COMPOSE up -d
else
  echo "  pin unchanged (nodes already enforce it)"
fi
echo "  DKMS_AUTHORITY_NODE_SET_ID_B64=${NODE_SET_ID_B64}"

cat <<EOF

QUORUM UP. Point a runtime at it:

  export ELASTOS_DKMS_REMOTE=1
  export ELASTOS_DKMS_CARRIER=1
  export ELASTOS_DKMS_REMOTE_DESCRIPTOR="$(pwd)/shared/dkms-authority.carrier.json"
  export ELASTOS_DKMS_REMOTE_CALLER_SEED="$(pwd)/shared/caller.seed"
  ./scripts/dev/run-creator-gateway.sh

Health:   docker compose ps && docker compose logs --tail=20
Logs:     debug-level node traces (every handshake/recover + outcome) also land in
          ~/Library/Application Support/ElastOS/dkms/docker-logs/node-{0,1,2}.log
          (tune with DKMS_AUTHORITY_LOG_LEVEL / DKMS_CARRIER_NODE_LOG_LEVEL: error|warn|info|debug|trace)
Restart:  master seeds live in named volumes — nodes come back with the SAME identities.
EOF
