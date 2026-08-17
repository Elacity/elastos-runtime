#!/usr/bin/env bash
#
# Prove the dkms-live-recover harness end to end against THREE LOCAL dkms-authority daemons
# (loopback TCP) before it is ever pointed at the live mesh. Spins three daemons with the
# SAME allow-listed caller, assembles a v2 descriptor from each node's published identity
# (`describe`), and runs the full 2-of-3 escrow -> recover -> combine. Non-destructive; cleans
# up the daemons + temp dir on exit.
#
# Usage: scripts/dev/dkms-live-recover/local-verify.sh
# Exit: 0 PASS / 1 FAIL.

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
CAP="$REPO/capsules"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/dkms-live-verify.XXXXXX")"
PIDS=()
cleanup() { for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null; done; rm -rf "$WORK"; }
trap cleanup EXIT

echo "== build dkms-authority + dkms-live-recover =="
cargo build --quiet --manifest-path "$CAP/dkms-authority/Cargo.toml" || { echo "FAIL build dkms-authority"; exit 1; }
cargo build --quiet --manifest-path "$HERE/Cargo.toml" || { echo "FAIL build harness"; exit 1; }
NODE_BIN="$CAP/dkms-authority/target/debug/dkms-authority"
HARNESS="$HERE/target/debug/dkms-live-recover"

# A throwaway allow-listed caller for the LOCAL run (the live run uses the real ~/.elastos-dkms seed).
# 32 random bytes, base64 -> the same seed format the harness consumes.
SEED_B64="$(head -c 32 /dev/urandom | base64)"
printf '%s' "$SEED_B64" > "$WORK/caller.seed"
# Derive the caller verifying key the daemons must allow-list: the harness exposes no keygen,
# so we let node A learn it from a dry connection is not possible; instead reuse dkms-keygen if
# present, else fall back to an empty allow-list (the daemon then serves any well-formed caller —
# acceptable for this LOCAL wire check; the live nodes enforce the real allow-list).
ALLOW=""
if [ -f "$CAP/dkms-keygen/Cargo.toml" ]; then
  cargo build --quiet --manifest-path "$CAP/dkms-keygen/Cargo.toml" 2>/dev/null && \
  ALLOW="$("$CAP/dkms-keygen/target/debug/dkms-keygen" derive-vk --seed-b64 "$SEED_B64" 2>/dev/null | tr -d '\n')"
fi
echo "  allow-list caller vk: ${ALLOW:+set}${ALLOW:-<empty: explicit-anonymous for local wire check>}"

# DKMS-8: caller policy is EXPLICIT — an empty allow-list is no longer a silent serve-any. When no
# caller vk was derived, declare anonymous via the opt-in; otherwise enforce the derived allow-list.
if [[ -n "$ALLOW" ]]; then
  POLICY_ENV=(DKMS_AUTHORITY_ALLOWED_CALLERS="$ALLOW")
else
  POLICY_ENV=(DKMS_AUTHORITY_ALLOW_ANONYMOUS=1)
fi

echo "== start three local daemons (loopback TCP) =="
PORTS=(19451 19452 19453)
ENDPOINTS=()
for i in 0 1 2; do
  P="${PORTS[$i]}"
  DKMS_AUTHORITY_LISTEN="tcp:127.0.0.1:$P" \
  DKMS_AUTHORITY_KEY_STORE="$WORK/node$i.json" \
  env "${POLICY_ENV[@]}" \
  DKMS_AUTHORITY_OPERATOR_VK="" \
    "$NODE_BIN" >/dev/null 2>"$WORK/node$i.log" &
  PIDS+=("$!")
  ENDPOINTS+=("tcp:127.0.0.1:$P")
done

# Wait for all three to accept.
for ep in "${ENDPOINTS[@]}"; do
  addr="${ep#tcp:}"; host="${addr%:*}"; port="${addr##*:}"
  for _ in $(seq 1 200); do
    (exec 3<>"/dev/tcp/$host/$port") 2>/dev/null && { exec 3>&- ; break; }
    sleep 0.05
  done
done

echo "== assemble a v2 descriptor from each node's published identity =="
N0="$("$HARNESS" describe "${ENDPOINTS[0]}")" || { echo "FAIL describe node0"; cat "$WORK"/node*.log; exit 1; }
N1="$("$HARNESS" describe "${ENDPOINTS[1]}")" || { echo "FAIL describe node1"; exit 1; }
N2="$("$HARNESS" describe "${ENDPOINTS[2]}")" || { echo "FAIL describe node2"; exit 1; }

DESC="$WORK/descriptor.json"
python3 - "$N0" "$N1" "$N2" > "$DESC" <<'PY'
import json, sys
n = [json.loads(a) for a in sys.argv[1:4]]
desc = {
  "schema": "elastos.dkms.authority/v2",
  "verifying_key_b64": n[0]["verifying_key_b64"],
  "recipient_pub_b64": n[0]["recipient_pub_b64"],
  "authority_endpoint": n[0]["authority_endpoint"],
  "threshold": {"t": 2, "nodes": n},
}
print(json.dumps(desc, indent=2))
PY
echo "  descriptor: $DESC"

echo "== run the harness against the three local daemons =="
"$HARNESS" "$DESC" "$WORK/caller.seed"
rc=$?
echo
if [ $rc -eq 0 ]; then echo "local-verify: PASS"; else echo "local-verify: FAIL ($rc)"; cat "$WORK"/node*.log; fi
exit $rc
