#!/usr/bin/env bash
#
# LIVE PRODUCER VERTICAL — local proof of the production wiring.
#
# Proves the PRODUCER half end to end against a 2-of-3 quorum the producer does NOT own:
#
#   encrypt[seal_inline_threshold]  ->  key-provider[dkms: recover 2-of-3 over live endpoints]
#       ->  decrypt[reconstruct in-VM + decrypt the segment sealed in THIS run]
#
# A CEK is minted RIGHT NOW inside encrypt-provider, CENC-encrypts fresh bytes, is Shamir-split,
# and each share is sealed to a NODE's published recipient (the CEK never assembled in the
# producer boundary). The key-provider (dkms backend) connects to the nodes as the allow-listed
# caller, recovers from ANY TWO, re-seals to the decrypt session; the decrypt boundary
# reconstructs the CEK in-VM and decrypts the freshly-sealed segment. No golden, no Lit.
#
# Here three local tcp daemons stand in for InterServer/Contabo/node3. The geographically-live
# run is THIS EXACT config with the production descriptor + the allow-listed caller seed — only
# the endpoints differ. The producer spawns NOTHING and performs NO destructive op.
#
# Exit: 0 on PASS, 1 on FAIL.

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
CAP="$REPO/capsules"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ddrm-live-producer.XXXXXX")"
PIDS=()
cleanup() { for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null; done; rm -rf "$WORK"; }
trap cleanup EXIT

echo "== build real capsules + node + describe helper =="
build() {
  local path="$1"; shift
  cargo build --quiet --manifest-path "$path/Cargo.toml" "$@" || { echo "FAIL build $path"; exit 1; }
}
build "$CAP/encrypt-provider" --features escrow
build "$CAP/key-provider" --features key-authority-ref
build "$CAP/decrypt-provider" --features rail-material,rail-mint
build "$CAP/dkms-authority"
build "$CAP/dkms-keygen"
build "$REPO/scripts/dev/dkms-live-recover"   # the `describe` helper
build "$HERE"                                 # ddrm-producer-smoke (live mode)

ENCRYPT_BIN="$CAP/encrypt-provider/target/debug/encrypt-provider"
KEY_BIN="$CAP/key-provider/target/debug/key-provider"
DECRYPT_BIN="$CAP/decrypt-provider/target/debug/decrypt-provider"
NODE_BIN="$CAP/dkms-authority/target/debug/dkms-authority"
KEYGEN="$CAP/dkms-keygen/target/debug/dkms-keygen"
DESCRIBE="$REPO/scripts/dev/dkms-live-recover/target/debug/dkms-live-recover"
SMOKE="$HERE/target/debug/ddrm-producer-smoke"

# The producer's ALLOW-LISTED caller seed — the same identity every node admits. In the live run
# this is ~/.elastos-dkms/secrets/caller.seed; here we mint a throwaway one and allow-list it.
SEED_B64="$(head -c 32 /dev/urandom | base64 | tr -d '\n')"
ALLOW="$("$KEYGEN" derive-vk --seed-b64 "$SEED_B64" | tr -d '\n')"
echo "  allow-list caller vk: set"

echo "== start three local daemons (loopback tcp) standing in for the live quorum =="
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

echo "== assemble the PUBLIC-ONLY v2 descriptor from each node's published identity =="
N0="$("$DESCRIBE" describe "${ENDPOINTS[0]}")" || { echo "FAIL describe node0"; cat "$WORK"/node*.log; exit 1; }
N1="$("$DESCRIBE" describe "${ENDPOINTS[1]}")" || { echo "FAIL describe node1"; exit 1; }
N2="$("$DESCRIBE" describe "${ENDPOINTS[2]}")" || { echo "FAIL describe node2"; exit 1; }
DESC="$WORK/dkms-authority.v2.json"
python3 - "$N0" "$N1" "$N2" > "$DESC" <<'PY'
import json, sys
n = [json.loads(a) for a in sys.argv[1:4]]
print(json.dumps({
  "schema": "elastos.dkms.authority/v2",
  "verifying_key_b64": n[0]["verifying_key_b64"],
  "recipient_pub_b64": n[0]["recipient_pub_b64"],
  "authority_endpoint": n[0]["authority_endpoint"],
  "threshold": {"t": 2, "nodes": n},
}, indent=2))
PY
echo "  descriptor: $DESC"

echo "== run the LIVE producer vertical (mint -> quorum escrow -> 2-of-3 recover -> decrypt) =="
echo
"$SMOKE" "$ENCRYPT_BIN" "$KEY_BIN" "$DECRYPT_BIN" --live "$DESC" "$SEED_B64"
rc=$?
echo
if [ $rc -eq 0 ]; then
  echo "live-producer-verify: PASS — a real CEK minted now, escrowed to an external 2-of-3 quorum, recovered 2-of-3, decrypted the segment sealed in this run"
else
  echo "live-producer-verify: FAIL ($rc)"; cat "$WORK"/node*.log
fi
exit $rc
