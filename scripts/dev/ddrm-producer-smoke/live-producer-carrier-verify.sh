#!/usr/bin/env bash
#
# LIVE PRODUCER VERTICAL OVER CARRIER — local proof of the dKMS-over-Carrier transport.
#
# Identical to live-producer-verify.sh (real encrypt/key/decrypt capsules + 3 real dkms-authority
# daemons + a real 2-of-3 recover), EXCEPT the runtime reaches each node over Carrier (iroh) by
# its did:key instead of a direct tcp/WireGuard endpoint:
#
#   key-provider[carrier:did] -> dkms-carrier-client (loopback) -> Carrier/iroh
#       -> dkms-carrier-node (per node) -> dkms-authority (tcp, unchanged)
#
# The PQ-hybrid encrypted, mutually-authenticated channel + Shamir 2-of-3 recover run BYTE-FOR-BYTE
# unchanged end to end; only packet routing differs. This is the full production wiring with the
# WireGuard hop swapped for Carrier — proven with ZERO production-node risk (3 local daemons stand
# in for InterServer/Contabo/node3; 3 local bridges stand in for their dkms-carrier-node services).
#
# Exit: 0 on PASS, 1 on FAIL.

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
CAP="$REPO/capsules"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ddrm-carrier.XXXXXX")"
PIDS=()
cleanup() { for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null; done; rm -rf "$WORK"; }
trap cleanup EXIT

echo "== build real capsules + node + bridges + describe helper =="
build() {
  local path="$1"; shift
  cargo build --quiet --manifest-path "$path/Cargo.toml" "$@" || { echo "FAIL build $path"; exit 1; }
}
build "$CAP/encrypt-provider" --features escrow
build "$CAP/key-provider" --features key-authority-ref
build "$CAP/decrypt-provider" --features rail-material,rail-mint
build "$CAP/dkms-authority"
build "$CAP/dkms-keygen"
build "$REPO/scripts/dev/dkms-live-recover"     # the `describe` helper
build "$REPO/scripts/dev/dkms-carrier-node"     # node-side Carrier bridge
build "$REPO/scripts/dev/dkms-carrier-client"   # runtime-side Carrier sidecar
build "$HERE"                                   # ddrm-producer-smoke (live mode)

ENCRYPT_BIN="$CAP/encrypt-provider/target/debug/encrypt-provider"
KEY_BIN="$CAP/key-provider/target/debug/key-provider"
DECRYPT_BIN="$CAP/decrypt-provider/target/debug/decrypt-provider"
NODE_BIN="$CAP/dkms-authority/target/debug/dkms-authority"
KEYGEN="$CAP/dkms-keygen/target/debug/dkms-keygen"
DESCRIBE="$REPO/scripts/dev/dkms-live-recover/target/debug/dkms-live-recover"
BRIDGE_NODE="$REPO/scripts/dev/dkms-carrier-node/target/debug/dkms-carrier-node"
BRIDGE_CLIENT="$REPO/scripts/dev/dkms-carrier-client/target/debug/dkms-carrier-client"
SMOKE="$HERE/target/debug/ddrm-producer-smoke"

SEED_B64="$(head -c 32 /dev/urandom | base64 | tr -d '\n')"
ALLOW="$("$KEYGEN" derive-vk --seed-b64 "$SEED_B64" | tr -d '\n')"
echo "  allow-list caller vk: set"

echo "== start three local dkms-authority daemons (loopback tcp) standing in for the live quorum =="
PORTS=(19471 19472 19473)
ENDPOINTS=()
for i in 0 1 2; do
  P="${PORTS[$i]}"
  DKMS_AUTHORITY_LISTEN="tcp:127.0.0.1:$P" \
  DKMS_AUTHORITY_KEY_STORE="$WORK/node$i.json" \
  DKMS_AUTHORITY_ALLOWED_CALLERS="$ALLOW" \
  DKMS_AUTHORITY_OPERATOR_VK="" \
    "$NODE_BIN" >/dev/null 2>"$WORK/node$i.log" &
  PIDS+=("$!")
  ENDPOINTS+=("tcp:127.0.0.1:$P")
done
for ep in "${ENDPOINTS[@]}"; do
  addr="${ep#tcp:}"; host="${addr%:*}"; port="${addr##*:}"
  for _ in $(seq 1 200); do
    (exec 3<>"/dev/tcp/$host/$port") 2>/dev/null && { exec 3>&- ; break; }
    sleep 0.05
  done
done

echo "== describe each node's PUBLISHED PQ identity (over tcp; pins are transport-independent) =="
N0="$("$DESCRIBE" describe "${ENDPOINTS[0]}")" || { echo "FAIL describe node0"; cat "$WORK"/node*.log; exit 1; }
N1="$("$DESCRIBE" describe "${ENDPOINTS[1]}")" || { echo "FAIL describe node1"; exit 1; }
N2="$("$DESCRIBE" describe "${ENDPOINTS[2]}")" || { echo "FAIL describe node2"; exit 1; }

echo "== start three dkms-carrier-node bridges (one per daemon) and capture their did:key =="
DIDS=()
for i in 0 1 2; do
  P="${PORTS[$i]}"
  DKMS_CARRIER_NODE_TARGET="127.0.0.1:$P" \
  DKMS_CARRIER_NODE_SEED="$WORK/bridge$i.seed" \
    "$BRIDGE_NODE" >"$WORK/bridge$i.out" 2>"$WORK/bridge$i.err" &
  PIDS+=("$!")
done
for i in 0 1 2; do
  DID=""
  for _ in $(seq 1 60); do
    DID="$(head -n1 "$WORK/bridge$i.out" 2>/dev/null)"
    [ -n "$DID" ] && break
    sleep 0.5
  done
  [ -z "$DID" ] && { echo "FAIL: bridge$i produced no did"; cat "$WORK/bridge$i.err"; exit 1; }
  DIDS+=("$DID")
  echo "  node$i did: $DID"
done

echo "== start the runtime-side Carrier sidecar =="
DKMS_CARRIER_CLIENT_LISTEN="127.0.0.1:9444" "$BRIDGE_CLIENT" \
  >"$WORK/client.out" 2>"$WORK/client.err" &
PIDS+=("$!")
for _ in $(seq 1 60); do grep -q '^ready' "$WORK/client.out" 2>/dev/null && break; sleep 0.5; done
grep -q '^ready' "$WORK/client.out" 2>/dev/null || { echo "FAIL: sidecar not ready"; cat "$WORK/client.err"; exit 1; }

echo "== assemble a CARRIER v2 descriptor: same PQ pins, endpoints rewritten to carrier:did =="
DESC="$WORK/dkms-authority.carrier.json"
python3 - "$N0" "$N1" "$N2" "${DIDS[0]}" "${DIDS[1]}" "${DIDS[2]}" > "$DESC" <<'PY'
import json, sys
n = [json.loads(a) for a in sys.argv[1:4]]
dids = sys.argv[4:7]
for node, did in zip(n, dids):
    node["authority_endpoint"] = f"carrier:{did}"
print(json.dumps({
  "schema": "elastos.dkms.authority/v2",
  "verifying_key_b64": n[0]["verifying_key_b64"],
  "recipient_pub_b64": n[0]["recipient_pub_b64"],
  "authority_endpoint": n[0]["authority_endpoint"],
  "threshold": {"t": 2, "nodes": n},
}, indent=2))
PY
echo "  descriptor: $DESC"

echo "== run the LIVE producer vertical OVER CARRIER (mint -> escrow -> 2-of-3 recover -> decrypt) =="
echo
DKMS_CARRIER_CLIENT_ADDR="127.0.0.1:9444" "$SMOKE" "$ENCRYPT_BIN" "$KEY_BIN" "$DECRYPT_BIN" --live "$DESC" "$SEED_B64"
rc=$?
echo
if [ $rc -eq 0 ]; then
  echo "live-producer-carrier-verify: PASS — real CEK minted, escrowed to a 2-of-3 quorum, recovered OVER CARRIER (did:key, no WireGuard), decrypted the segment sealed in this run"
else
  echo "live-producer-carrier-verify: FAIL ($rc)"
  echo "--- node logs ---"; cat "$WORK"/node*.log 2>/dev/null
  echo "--- bridge errs ---"; tail -3 "$WORK"/bridge*.err 2>/dev/null
  echo "--- client err ---"; tail -5 "$WORK/client.err" 2>/dev/null
fi
exit $rc
