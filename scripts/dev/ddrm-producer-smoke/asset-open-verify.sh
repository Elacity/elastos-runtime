#!/usr/bin/env bash
#
# ASSET OPEN VERTICAL — local proof that a minted asset is RECOVERABLE from its persisted escrow.
#
# This is the runtime's Library-open path distilled to its crypto core, run end to end against a
# 2-of-3 quorum the producer does NOT own:
#
#   phase A (MINT):  encrypt[seal_inline_threshold] on a REAL file
#                    -> PERSIST { escrow.json (the protections[0] envelope), ciphertext.bin }
#   phase B (OPEN):  reload escrow.json + ciphertext.bin FROM DISK ALONE
#                    -> key-provider[dkms: recover 2-of-3 over live endpoints]
#                    -> decrypt[reconstruct CEK in-VM + decrypt the segment]
#
# Between the phases every in-memory copy of the seal output is dropped — phase B reads only the
# persisted sidecar, exactly as `/api/viewers/open` reads a stored owned object. This is the proof
# that the keystone fix (persisting `producer_verifying_key_b64` into the mint envelope) makes
# minted assets RECOVERABLE: with nothing held in process, the quorum still validates the share
# signatures and reconstructs the CEK.
#
# Three local tcp daemons stand in for the live quorum; the geographically-live run is THIS EXACT
# config with the production descriptor + allow-listed caller seed — only the endpoints differ.
#
# Usage:  asset-open-verify.sh [path-to-asset-file]
#         (defaults to a generated PNG-ish test asset if no file is given)
#
# Exit: 0 on PASS, 1 on FAIL.

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
CAP="$REPO/capsules"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ddrm-asset-open.XXXXXX")"
PIDS=()
cleanup() { for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null; done; rm -rf "$WORK"; }
trap cleanup EXIT

# --- resolve the asset to seal+open (a real file, or a generated test asset). ---
ASSET="${1:-}"
if [ -z "$ASSET" ]; then
  ASSET="$WORK/test-asset.bin"
  # A non-trivial, non-compressible blob so "byte-identical decrypt" is a real claim.
  head -c 65536 /dev/urandom > "$ASSET"
  echo "== no asset given; generated a 64KiB random test asset =="
fi
[ -f "$ASSET" ] || { echo "FAIL: asset not found: $ASSET"; exit 1; }
echo "== asset: $ASSET ($(wc -c < "$ASSET" | tr -d ' ') bytes) =="

echo "== build real capsules + node + describe helper =="
build() {
  local path="$1"; shift
  cargo build --quiet --manifest-path "$path/Cargo.toml" "$@" || { echo "FAIL build $path"; exit 1; }
}
build "$CAP/encrypt-provider" --features escrow
build "$CAP/key-provider" --features key-authority-ref
build "$CAP/decrypt-provider" --features rail-stream,rail-mint
build "$CAP/dkms-authority"
build "$CAP/dkms-keygen"
build "$REPO/scripts/dev/dkms-live-recover"
build "$HERE"

ENCRYPT_BIN="$CAP/encrypt-provider/target/debug/encrypt-provider"
KEY_BIN="$CAP/key-provider/target/debug/key-provider"
DECRYPT_BIN="$CAP/decrypt-provider/target/debug/decrypt-provider"
NODE_BIN="$CAP/dkms-authority/target/debug/dkms-authority"
KEYGEN="$CAP/dkms-keygen/target/debug/dkms-keygen"
DESCRIBE="$REPO/scripts/dev/dkms-live-recover/target/debug/dkms-live-recover"
SMOKE="$HERE/target/debug/ddrm-producer-smoke"

SEED_B64="$(head -c 32 /dev/urandom | base64 | tr -d '\n')"
ALLOW="$("$KEYGEN" derive-vk --seed-b64 "$SEED_B64" | tr -d '\n')"
echo "  allow-list caller vk: set"

echo "== start three local daemons (loopback tcp) standing in for the live quorum =="
PORTS=(19481 19482 19483)
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

echo "== run the ASSET open vertical (seal real file -> persist escrow -> reload -> 2-of-3 recover -> decrypt) =="
echo
"$SMOKE" "$ENCRYPT_BIN" "$KEY_BIN" "$DECRYPT_BIN" --asset "$ASSET" "$DESC" "$SEED_B64"
rc=$?
echo
if [ $rc -eq 0 ]; then
  echo "asset-open-verify: PASS — a real asset, sealed to a 2-of-3 quorum and PERSISTED to disk, was reloaded from disk alone, recovered 2-of-3, decrypted, and RENDERED byte-identical (full consumer-open path)"
else
  echo "asset-open-verify: FAIL ($rc)"; cat "$WORK"/node*.log
fi
exit $rc
