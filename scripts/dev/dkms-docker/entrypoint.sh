#!/usr/bin/env bash
# Per-container boot for ONE simulated dKMS quorum node. Mirrors a live node:
#   1. one-shot `init` — idempotently creates /data/node.store.json (the master seed;
#      the container's stand-in for /var/lib/elastos/dkms/master.seed) and publishes
#      the node's PUBLIC identity (verifying key + escrow recipient) to /shared.
#   2. the dkms-authority daemon on tcp:0.0.0.0:9443 (live nodes bind the private mesh;
#      here the container network IS the private network).
#   3. the dkms-carrier-node bridge fronting it — an untrusted ciphertext relay that
#      prints this node's stable `did:key` (captured to /shared for the descriptor).
#
# /data is the node's PRIVATE volume (never leaves the container: master seed, carrier
# identity seed). /shared carries ONLY public artifacts (identities, dids).
set -euo pipefail

IDX="${NODE_INDEX:?NODE_INDEX (0/1/2) is required}"
STORE=/data/node.store.json
mkdir -p /data /shared
chmod 700 /data

echo "node${IDX}: init (create-or-load master seed store) ..."
INIT_RESP="$(printf '%s\n%s\n' \
  "{\"op\":\"init\",\"config\":{\"authority_key_store\":\"${STORE}\"}}" \
  '{"op":"shutdown"}' \
  | dkms-authority | head -n 1)"
if [ -z "$INIT_RESP" ] || ! printf '%s' "$INIT_RESP" | grep -q '"status":"ok"'; then
  echo "node${IDX}: FAIL — init did not publish an identity: ${INIT_RESP}" >&2
  exit 1
fi
printf '%s\n' "$INIT_RESP" > "/shared/node-${IDX}.init.json"
echo "node${IDX}: public identity -> /shared/node-${IDX}.init.json"

echo "node${IDX}: starting dkms-authority on tcp:0.0.0.0:9443 ..."
DKMS_AUTHORITY_LISTEN="tcp:0.0.0.0:9443" \
DKMS_AUTHORITY_KEY_STORE="$STORE" \
  dkms-authority &
AUTH_PID=$!

for _ in $(seq 1 100); do
  (exec 3<>/dev/tcp/127.0.0.1/9443) 2>/dev/null && { exec 3>&- || true; break; }
  sleep 0.1
done

echo "node${IDX}: starting dkms-carrier-node bridge ..."
rm -f "/shared/node-${IDX}.did"
DKMS_CARRIER_NODE_TARGET="127.0.0.1:9443" \
DKMS_CARRIER_NODE_SEED="/data/carrier.seed" \
  dkms-carrier-node > "/shared/node-${IDX}.did" &
BRIDGE_PID=$!

for _ in $(seq 1 120); do
  DID="$(head -n 1 "/shared/node-${IDX}.did" 2>/dev/null || true)"
  [ -n "$DID" ] && { echo "node${IDX}: carrier did: ${DID}"; break; }
  sleep 0.5
done

trap 'kill "$AUTH_PID" "$BRIDGE_PID" 2>/dev/null || true' TERM INT
wait -n "$AUTH_PID" "$BRIDGE_PID"
echo "node${IDX}: a component exited — stopping container" >&2
exit 1
