#!/usr/bin/env bash
#
# LIVE-AUTHORITIES OPEN — local proof of the production wiring.
#
# Proves the runtime-core open path (`ddrm-runtime-open`) drives the FULL consumer chain
#
#     drm/open -> rights (real chain-provider) -> key (REAL key-provider) -> decrypt (REAL CENC segment)
#
# against a 2-of-3 quorum it does NOT own — i.e. the `authority.live_descriptor` mode: the nodes are
# ALREADY running (here: three local tcp daemons standing in for InterServer/Contabo/node3), the
# runtime provisions/spawns NOTHING, reads each node's pinned identity + endpoint from a published
# PUBLIC-ONLY descriptor, escrows the CEK shares to those recipients, and DELEGATES recovery over the
# real endpoints. This is the SAME code path that will run on `node3` against the live quorum — only
# the descriptor's endpoints + the allow-listed caller seed differ.
#
# This is the OFFLINE-verifiable half of the live close-the-loop: real capsules, real decrypt, real
# external endpoints. The geographically-live run is this exact config with the production descriptor.
#
# Exit: 0 on PASS, 1 on FAIL.

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
CAP="$REPO/capsules"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ddrm-live-open.XXXXXX")"
PIDS=()
cleanup() { for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null; done; rm -rf "$WORK"; }
trap cleanup EXIT

echo "== build real capsules + node + describe helper =="
build() {
  local path="$1"; shift
  cargo build --quiet --manifest-path "$path/Cargo.toml" "$@" || { echo "FAIL build $path"; exit 1; }
}
build "$CAP/key-provider" --features key-authority-ref
build "$CAP/decrypt-provider" --features rail-material,rail-mint
build "$CAP/drm-provider"
build "$CAP/rights-provider" --features chain-rights
build "$CAP/chain-provider"
build "$CAP/dkms-authority"
build "$CAP/dkms-keygen"
build "$HERE"                       # the orchestrator (live mode)
build "$REPO/scripts/dev/dkms-live-recover"   # the `describe` helper

KEY_BIN="$CAP/key-provider/target/debug/key-provider"
DECRYPT_BIN="$CAP/decrypt-provider/target/debug/decrypt-provider"
DRM_BIN="$CAP/drm-provider/target/debug/drm-provider"
RIGHTS_BIN="$CAP/rights-provider/target/debug/rights-provider"
CHAIN_BIN="$CAP/chain-provider/target/debug/chain-provider"
NODE_BIN="$CAP/dkms-authority/target/debug/dkms-authority"
KEYGEN="$CAP/dkms-keygen/target/debug/dkms-keygen"
DESCRIBE="$REPO/scripts/dev/dkms-live-recover/target/debug/dkms-live-recover"
ORCH="$HERE/target/debug/ddrm-runtime-open"

# The runtime's ALLOW-LISTED caller seed — the same identity every node admits. In the live run this
# is ~/.elastos-dkms/secrets/caller.seed; here we mint a throwaway one and allow-list it on the daemons.
SEED_B64="$(head -c 32 /dev/urandom | base64)"
printf '%s' "$SEED_B64" > "$WORK/caller.seed"
ALLOW="$("$KEYGEN" derive-vk --seed-b64 "$SEED_B64" | tr -d '\n')"
echo "  allow-list caller vk: set"

echo "== start three local daemons (loopback tcp) standing in for the live quorum =="
PORTS=(19461 19462 19463)
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

echo "== run the REAL runtime-core open in live-authorities mode =="
CONFIG="$WORK/open-config.json"
cat > "$CONFIG" <<JSON
{
  "mode": "open",
  "authority": {
    "backend": "dkms",
    "threshold": true,
    "nodes": 3,
    "transport": "tcp",
    "live_descriptor": "$DESC",
    "dkms_caller_seed_path": "$WORK/caller.seed"
  },
  "key_bin": "$KEY_BIN",
  "decrypt_bin": "$DECRYPT_BIN",
  "drm_bin": "$DRM_BIN",
  "rights_bin": "$RIGHTS_BIN",
  "chain_bin": "$CHAIN_BIN",
  "work_dir": "$WORK/run"
}
JSON
echo "  config: $CONFIG"
echo
"$ORCH" "$CONFIG"
rc=$?
echo
if [ $rc -eq 0 ]; then
  echo "live-open-verify: PASS — real capsules decrypted a real segment with a CEK recovered from an external 2-of-3 quorum"
else
  echo "live-open-verify: FAIL ($rc)"; cat "$WORK"/node*.log
fi
exit $rc
