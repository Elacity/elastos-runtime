#!/usr/bin/env bash
#
# QUORUM GATEWAY VERIFY — prove the EXACT operator path run-creator-gateway.sh sets up:
#   * provision a persistent 2-of-3 quorum (the same ddrm-provision-quorum.sh the gateway calls);
#   * start the 3 node daemons from the PRIVATE sidecar on their UNIX sockets (durable stores);
#   * assemble the v2 OPEN descriptor (mint identities + live endpoints) + a caller seed;
#   * MINT a `.ddrm` capsule using the PUBLIC v1 descriptor (what the Create portal seals to);
#   * OPEN it with `ddrm-media-authority --quorum` against the v2 descriptor over the UNIX sockets;
#   * assert byte-identical.
#
# This is stricter than quorum-helper-verify.sh: it exercises the unix-socket transport AND proves
# the v1-mint-descriptor identities match the v2-open-descriptor identities (the gateway's seam).

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
CAP="$REPO/capsules"
SMOKE_DIR="$REPO/scripts/dev/ddrm-producer-smoke"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ddrm-quorum-gw.XXXXXX")"
PIDS=()
cleanup() { for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null; done; rm -rf "$WORK"; }
trap cleanup EXIT

ASSET="$WORK/asset.bin"; head -c 40960 /dev/urandom > "$ASSET"
echo "== asset: 40KiB random =="

echo "== build capsules + helper =="
build() { cargo build --quiet --manifest-path "$1/Cargo.toml" "${@:2}" || { echo "FAIL build $1"; exit 1; }; }
build "$CAP/encrypt-provider" --features escrow
build "$CAP/key-provider" --features key-authority-ref
build "$CAP/decrypt-provider" --features rail-stream,rail-mint
build "$CAP/dkms-authority"
build "$CAP/dkms-keygen"
build "$SMOKE_DIR"
build "$HERE"

ENCRYPT_BIN="$CAP/encrypt-provider/target/debug/encrypt-provider"
KEY_BIN="$CAP/key-provider/target/debug/key-provider"
DECRYPT_BIN="$CAP/decrypt-provider/target/debug/decrypt-provider"
NODE_BIN="$CAP/dkms-authority/target/debug/dkms-authority"
KEYGEN="$CAP/dkms-keygen/target/debug/dkms-keygen"
SMOKE="$SMOKE_DIR/target/debug/ddrm-producer-smoke"
HELPER="$HERE/target/debug/ddrm-media-authority"

QDIR="$WORK/dkms"
echo "== provision the persistent 2-of-3 quorum (the gateway's provisioning step) =="
bash "$REPO/scripts/dev/ddrm-provision-quorum.sh" "$QDIR" >"$WORK/provision.log" 2>&1 || { echo "FAIL provision"; cat "$WORK/provision.log"; exit 1; }
QUORUM_JSON="$QDIR/quorum.json"
QUORUM_NODES="$QDIR/quorum-nodes.json"
[ -f "$QUORUM_JSON" ] && [ -f "$QUORUM_NODES" ] || { echo "FAIL: provisioning artifacts missing"; exit 1; }

echo "== start the 3 node daemons from the sidecar on their UNIX sockets =="
SEED_B64="$(head -c 32 /dev/urandom | base64 | tr -d '\n')"
CALLER_VK="$("$KEYGEN" derive-vk --seed-b64 "$SEED_B64" | tr -d '\n')"
[ -n "$CALLER_VK" ] || { echo "FAIL: could not derive caller vk"; exit 1; }

NODE_LINES=()
while IFS= read -r ln; do [ -n "$ln" ] && NODE_LINES+=("$ln"); done < <(python3 - "$QUORUM_NODES" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
for n in d.get("nodes", []):
    print("\t".join([n["verifying_key_b64"], n["recipient_pub_b64"], n["key_store"], n["authority_endpoint"]]))
PY
)
[ "${#NODE_LINES[@]}" -eq 3 ] || { echo "FAIL: sidecar did not yield 3 nodes"; exit 1; }

VKS=(); RECS=(); ENDPOINTS=()
for ln in "${NODE_LINES[@]}"; do
  IFS=$'\t' read -r vk rec store endpoint <<<"$ln"
  VKS+=("$vk"); RECS+=("$rec"); ENDPOINTS+=("$endpoint")
  DKMS_AUTHORITY_LISTEN="$endpoint" DKMS_AUTHORITY_KEY_STORE="$store" \
    DKMS_AUTHORITY_ALLOWED_CALLERS="$CALLER_VK" \
    "$NODE_BIN" >/dev/null 2>>"$WORK/daemon.log" &
  PIDS+=("$!")
done
for ep in "${ENDPOINTS[@]}"; do
  for _ in $(seq 1 200); do [ -S "$ep" ] && break; sleep 0.05; done
  [ -S "$ep" ] || { echo "FAIL: node socket never appeared: $ep"; cat "$WORK/daemon.log"; exit 1; }
done

echo "== assemble the v2 OPEN descriptor (mint identities + live unix endpoints) =="
OPEN_DESC="$QDIR/quorum-open.json"
python3 - "$OPEN_DESC" "${VKS[@]}" "__SEP__" "${RECS[@]}" "__SEP__" "${ENDPOINTS[@]}" <<'PY'
import json, sys
args = sys.argv[1:]; out = args[0]; rest = args[1:]
groups, cur = [], []
for a in rest:
    if a == "__SEP__": groups.append(cur); cur = []
    else: cur.append(a)
groups.append(cur)
vks, recs, eps = groups
nodes = [{"verifying_key_b64": v, "recipient_pub_b64": r, "authority_endpoint": e} for v, r, e in zip(vks, recs, eps)]
json.dump({"schema":"elastos.dkms.authority/v2","verifying_key_b64":nodes[0]["verifying_key_b64"],
  "recipient_pub_b64":nodes[0]["recipient_pub_b64"],"authority_endpoint":nodes[0]["authority_endpoint"],
  "threshold":{"t":2,"n":len(nodes),"nodes":nodes}}, open(out,"w"), indent=2)
PY

echo "== MINT a .ddrm capsule sealing to the PUBLIC v1 descriptor (the Create portal's seam) =="
CAPSULE="$WORK/asset.ddrm"
"$SMOKE" "$ENCRYPT_BIN" --mint-capsule "$ASSET" "$QUORUM_JSON" "$CAPSULE" || { echo "FAIL mint-capsule"; exit 1; }

echo "== OPEN with ddrm-media-authority --quorum against the v2 descriptor (unix sockets) =="
python3 - "$HELPER" "$DECRYPT_BIN" "$KEY_BIN" "$CAPSULE" "$OPEN_DESC" "$SEED_B64" "$ASSET" <<'PY'
import base64, hashlib, json, subprocess, sys
helper, decrypt_bin, key_bin, capsule, desc, seed, asset = sys.argv[1:8]
p = subprocess.Popen([helper,"--quorum","--principal","person:local:gw-verify",
  "--decrypt-bin",decrypt_bin,"--key-bin",key_bin,"--capsule",capsule,"--descriptor",desc,
  "--caller-seed",seed,"--object-cid","owned:gw-verify","--mime","application/octet-stream"],
  stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
try:
    dline = p.stdout.readline()
    if not dline: print("FAIL: no descriptor from helper"); sys.exit(1)
    p.stdin.write(json.dumps({"op":"object"})+"\n"); p.stdin.flush()
    resp = json.loads(p.stdout.readline())
    if resp.get("status") != "ok": print(f"FAIL: object op: {resp}"); sys.exit(1)
    got = base64.b64decode(resp["object_b64"])
    p.stdin.write(json.dumps({"op":"shutdown"})+"\n"); p.stdin.flush()
finally:
    p.wait(timeout=10)
want = open(asset,"rb").read()
if got != want:
    print(f"FAIL: {len(got)} bytes != {len(want)} bytes"); sys.exit(1)
print(f"OK: gateway-path open rendered {len(got)} bytes BYTE-IDENTICAL (sha256={hashlib.sha256(got).hexdigest()[:16]}…)")
PY
rc=$?
echo
if [ $rc -eq 0 ]; then
  echo "quorum-gateway-verify: PASS — the run-creator-gateway.sh open path (provision -> live unix-socket daemons -> v2 descriptor -> --quorum helper) recovers a minted dKMS asset byte-identically"
else
  echo "quorum-gateway-verify: FAIL ($rc)"; cat "$WORK/daemon.log" 2>/dev/null
fi
exit $rc
