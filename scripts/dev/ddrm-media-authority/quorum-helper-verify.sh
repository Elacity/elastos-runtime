#!/usr/bin/env bash
#
# QUORUM HELPER VERIFY — prove the PRODUCTION open helper (`ddrm-media-authority --quorum`)
# recovers a minted dKMS asset byte-identically through the live 2-of-3 quorum.
#
# This exercises the EXACT subprocess the gateway's /api/viewers/open spawns:
#   1. stand up 3 local dkms-authority daemons (stand-in for the live quorum) + a caller seed;
#   2. MINT a real asset to that quorum, writing the `.ddrm` capsule the gateway persists
#      (protections[0] escrow + ciphertext_b64) — via `ddrm-producer-smoke --mint-capsule`;
#   3. spawn `ddrm-media-authority --quorum` against that capsule + descriptor + caller seed;
#   4. drive its stdio protocol ({"op":"object"}) and assert the returned object_b64 decodes
#      BYTE-IDENTICAL to the original asset.
#
# Usage:  quorum-helper-verify.sh [path-to-asset-file]   (defaults to a 48KiB random asset)
# Exit:   0 on PASS, 1 on FAIL.

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
CAP="$REPO/capsules"
SMOKE_DIR="$REPO/scripts/dev/ddrm-producer-smoke"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ddrm-quorum-helper.XXXXXX")"
PIDS=()
cleanup() { for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null; done; rm -rf "$WORK"; }
trap cleanup EXIT

ASSET="${1:-}"
if [ -z "$ASSET" ]; then
  ASSET="$WORK/asset.bin"
  head -c 49152 /dev/urandom > "$ASSET"
  echo "== no asset given; generated a 48KiB random test asset =="
fi
[ -f "$ASSET" ] || { echo "FAIL: asset not found: $ASSET"; exit 1; }
echo "== asset: $ASSET ($(wc -c < "$ASSET" | tr -d ' ') bytes) =="

echo "== build capsules + helper =="
build() { cargo build --quiet --manifest-path "$1/Cargo.toml" "${@:2}" || { echo "FAIL build $1"; exit 1; }; }
build "$CAP/encrypt-provider" --features escrow
build "$CAP/key-provider" --features key-authority-ref
build "$CAP/decrypt-provider" --features rail-stream,rail-mint
build "$CAP/dkms-authority"
build "$CAP/dkms-keygen"
build "$REPO/scripts/dev/dkms-live-recover"
build "$SMOKE_DIR"
build "$HERE"

ENCRYPT_BIN="$CAP/encrypt-provider/target/debug/encrypt-provider"
KEY_BIN="$CAP/key-provider/target/debug/key-provider"
DECRYPT_BIN="$CAP/decrypt-provider/target/debug/decrypt-provider"
NODE_BIN="$CAP/dkms-authority/target/debug/dkms-authority"
KEYGEN="$CAP/dkms-keygen/target/debug/dkms-keygen"
DESCRIBE="$REPO/scripts/dev/dkms-live-recover/target/debug/dkms-live-recover"
SMOKE="$SMOKE_DIR/target/debug/ddrm-producer-smoke"
HELPER="$HERE/target/debug/ddrm-media-authority"

SEED_B64="$(head -c 32 /dev/urandom | base64 | tr -d '\n')"
ALLOW="$("$KEYGEN" derive-vk --seed-b64 "$SEED_B64" | tr -d '\n')"

echo "== start three local daemons (loopback tcp) standing in for the live quorum =="
PORTS=(19581 19582 19583)
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

echo "== assemble the PUBLIC-ONLY descriptor from each node's published identity =="
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

echo "== MINT: seal the asset to the quorum + write the .ddrm capsule the gateway persists =="
CAPSULE="$WORK/asset.ddrm"
"$SMOKE" "$ENCRYPT_BIN" --mint-capsule "$ASSET" "$DESC" "$CAPSULE" || { echo "FAIL mint-capsule"; exit 1; }

echo "== OPEN: spawn ddrm-media-authority --quorum and drive its stdio object protocol =="
python3 - "$HELPER" "$DECRYPT_BIN" "$KEY_BIN" "$CAPSULE" "$DESC" "$SEED_B64" "$ASSET" <<'PY'
import base64, hashlib, json, subprocess, sys

helper, decrypt_bin, key_bin, capsule, desc, seed, asset = sys.argv[1:8]
proc = subprocess.Popen(
    [helper, "--quorum",
     "--principal", "person:local:quorum-verify",
     "--decrypt-bin", decrypt_bin,
     "--key-bin", key_bin,
     "--capsule", capsule,
     "--descriptor", desc,
     "--caller-seed", seed,
     "--object-cid", "owned:verify-asset",
     "--mime", "application/octet-stream"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=None, text=True,
)
try:
    descriptor_line = proc.stdout.readline()
    if not descriptor_line:
        print("FAIL: helper printed no descriptor line"); sys.exit(1)
    d = json.loads(descriptor_line)
    if d.get("kind") != "object":
        print(f"FAIL: unexpected descriptor: {d}"); sys.exit(1)
    proc.stdin.write(json.dumps({"op": "object"}) + "\n"); proc.stdin.flush()
    resp_line = proc.stdout.readline()
    resp = json.loads(resp_line)
    if resp.get("status") != "ok":
        print(f"FAIL: helper object op errored: {resp}"); sys.exit(1)
    got = base64.b64decode(resp["object_b64"])
    proc.stdin.write(json.dumps({"op": "shutdown"}) + "\n"); proc.stdin.flush()
finally:
    proc.wait(timeout=10)

want = open(asset, "rb").read()
if got != want:
    print(f"FAIL: rendered {len(got)} bytes != original {len(want)} bytes "
          f"(sha256 got={hashlib.sha256(got).hexdigest()[:16]} want={hashlib.sha256(want).hexdigest()[:16]})")
    sys.exit(1)
print(f"OK: helper rendered {len(got)} bytes BYTE-IDENTICAL to the original "
      f"(sha256={hashlib.sha256(got).hexdigest()[:16]}…)")
PY
rc=$?
echo
if [ $rc -eq 0 ]; then
  echo "quorum-helper-verify: PASS — ddrm-media-authority --quorum recovered a minted dKMS asset byte-identically via the 2-of-3 quorum (the exact subprocess /api/viewers/open spawns)"
else
  echo "quorum-helper-verify: FAIL ($rc)"; cat "$WORK"/node*.log 2>/dev/null
fi
exit $rc
