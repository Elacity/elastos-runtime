#!/usr/bin/env bash
#
# Prove that two sessions on the same runtime can exchange gossip messages.
# This is the core invariant for native↔WASM chat interop.
#
# No WASM, no TUI, no pty — just API calls via curl.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ELASTOS="${ROOT}/elastos/target/debug/elastos"
TEST_HOME="$(mktemp -d /tmp/elastos-gossip-proof-XXXXXX)"

[[ -x "$ELASTOS" ]] || { echo "Build first: cd elastos && cargo build -p elastos-server" >&2; exit 1; }

# Setup minimal environment
mkdir -p "$TEST_HOME/xdg-data/elastos/bin"
for name in shell localhost-provider did-provider; do
    src=$(find "$ROOT/elastos/target/release" "$ROOT/capsules/did-provider/target/release" -name "$name" -type f -executable 2>/dev/null | head -1)
    [[ -n "$src" ]] && cp "$src" "$TEST_HOME/xdg-data/elastos/bin/$name"
done

# Build components manifest
PLATFORM="linux-$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')"
python3 - "$ROOT" "$TEST_HOME" "$PLATFORM" <<'PY'
import hashlib, json, pathlib, sys
root, home, platform = sys.argv[1], sys.argv[2], sys.argv[3]
source = pathlib.Path(root) / "components.json"
dest = pathlib.Path(home) / "xdg-data/elastos/components.json"
manifest = json.loads(source.read_text())
for name in ("shell", "localhost-provider", "did-provider"):
    path = dest.parent / "bin" / name
    if not path.exists(): continue
    blob = path.read_bytes()
    info = manifest["external"][name]["platforms"][platform]
    info["install_path"] = f"bin/{name}"
    info["checksum"] = "sha256:" + hashlib.sha256(blob).hexdigest()
    info["size"] = len(blob)
dest.parent.mkdir(parents=True, exist_ok=True)
dest.write_text(json.dumps(manifest, indent=2))
PY

cleanup() {
    [[ -f "$TEST_HOME/xdg-data/elastos/runtime-coords.json" ]] && {
        local pid
        pid=$(python3 -c "import json; print(json.load(open('$TEST_HOME/xdg-data/elastos/runtime-coords.json')).get('pid',''))" 2>/dev/null || true)
        [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true
    }
    rm -rf "$TEST_HOME"
}
trap cleanup EXIT

# Start managed runtime
echo "[proof] starting managed runtime..."
HOME="$TEST_HOME" \
XDG_DATA_HOME="$TEST_HOME/xdg-data" \
ELASTOS_DATA_DIR="$TEST_HOME/xdg-data/elastos" \
ELASTOS_QUIET_RUNTIME_NOTICES=1 \
"$ELASTOS" chat --nick probe <<< "/quit" &
CHAT_PID=$!

# Wait for runtime coords
COORDS="$TEST_HOME/xdg-data/elastos/runtime-coords.json"
for i in $(seq 1 30); do
    [[ -f "$COORDS" ]] && break
    sleep 1
done
[[ -f "$COORDS" ]] || { echo "FAIL: runtime did not start"; exit 1; }
wait $CHAT_PID 2>/dev/null || true

API_URL=$(python3 -c "import json; print(json.load(open('$COORDS'))['api_url'])")
ATTACH_SECRET=$(python3 -c "import json; print(json.load(open('$COORDS'))['attach_secret'])")
echo "[proof] runtime at $API_URL"

# Attach session A (simulates native chat)
TOKEN_A=$(curl -sf -X POST "$API_URL/api/auth/attach" \
  -H "Content-Type: application/json" \
  -d "{\"secret\":\"$ATTACH_SECRET\",\"scope\":\"client\"}" | python3 -c "import sys,json; print(json.load(sys.stdin)['token'])")
echo "[proof] session A attached"

# Attach session B (simulates WASM chat)
TOKEN_B=$(curl -sf -X POST "$API_URL/api/auth/attach" \
  -H "Content-Type: application/json" \
  -d "{\"secret\":\"$ATTACH_SECRET\",\"scope\":\"client\"}" | python3 -c "import sys,json; print(json.load(sys.stdin)['token'])")
echo "[proof] session B attached"

# Acquire peer capability for each session
acquire_cap() {
    local bearer="$1" resource="$2"
    # Request capability
    local req_resp
    req_resp=$(curl -sf -X POST "$API_URL/api/capability/request" \
      -H "Authorization: Bearer $bearer" \
      -H "Content-Type: application/json" \
      -d "{\"resource\":\"$resource\",\"action\":\"execute\"}")

    # Check if auto-granted
    local token
    token=$(echo "$req_resp" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('token',''))" 2>/dev/null)
    if [[ -n "$token" ]]; then echo "$token"; return; fi

    # Poll for grant
    local req_id
    req_id=$(echo "$req_resp" | python3 -c "import sys,json; print(json.load(sys.stdin).get('request_id',''))" 2>/dev/null)
    for i in $(seq 1 20); do
        sleep 0.2
        local status_resp
        status_resp=$(curl -sf "$API_URL/api/capability/request/$req_id" -H "Authorization: Bearer $bearer" 2>/dev/null || true)
        token=$(echo "$status_resp" | python3 -c "import sys,json; print(json.load(sys.stdin).get('token',''))" 2>/dev/null)
        if [[ -n "$token" ]]; then echo "$token"; return; fi
    done
    echo ""
}

PEER_CAP_A=$(acquire_cap "$TOKEN_A" "elastos://peer/*")
[[ -n "$PEER_CAP_A" ]] || { echo "FAIL: could not acquire peer cap for A"; exit 1; }
echo "[proof] A has peer capability"

PEER_CAP_B=$(acquire_cap "$TOKEN_B" "elastos://peer/*")
[[ -n "$PEER_CAP_B" ]] || { echo "FAIL: could not acquire peer cap for B"; exit 1; }
echo "[proof] B has peer capability"

# Both join #general
for CAP in "$PEER_CAP_A" "$PEER_CAP_B"; do
    # Find the matching bearer token
    if [[ "$CAP" == "$PEER_CAP_A" ]]; then BEARER="$TOKEN_A"; else BEARER="$TOKEN_B"; fi
    RESULT=$(curl -sf -X POST "$API_URL/api/provider/peer/gossip_join" \
      -H "Authorization: Bearer $BEARER" \
      -H "X-Capability-Token: $CAP" \
      -H "Content-Type: application/json" \
      -d '{"topic":"#general"}')
    STATUS=$(echo "$RESULT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',''))")
    [[ "$STATUS" == "ok" || "$RESULT" == *"already_joined"* ]] || { echo "FAIL: gossip_join: $RESULT"; exit 1; }
done
echo "[proof] both sessions joined #general"

# Session A sends a message
SEND_RESULT=$(curl -sf -X POST "$API_URL/api/provider/peer/gossip_send" \
  -H "Authorization: Bearer $TOKEN_A" \
  -H "X-Capability-Token: $PEER_CAP_A" \
  -H "Content-Type: application/json" \
  -d '{"topic":"#general","message":"hello-from-A","sender":"alice","sender_id":"did:test:A","sender_session_id":"session-A","ts":1000}')
echo "[proof] A sent: $SEND_RESULT"

# Session B reads with different consumer_id
sleep 1
RECV_RESULT=$(curl -sf -X POST "$API_URL/api/provider/peer/gossip_recv" \
  -H "Authorization: Bearer $TOKEN_B" \
  -H "X-Capability-Token: $PEER_CAP_B" \
  -H "Content-Type: application/json" \
  -d '{"topic":"#general","limit":50,"consumer_id":"consumer-B"}')

# Check if message is there
HAS_MESSAGE=$(echo "$RECV_RESULT" | python3 -c "
import sys, json
data = json.load(sys.stdin)
messages = data.get('data', {}).get('messages', [])
for m in messages:
    if m.get('content') == 'hello-from-A':
        print('YES')
        break
else:
    print('NO')
")

if [[ "$HAS_MESSAGE" == "YES" ]]; then
    echo "[proof] B received A's message: PASS"
else
    echo "[proof] B did NOT receive A's message: FAIL"
    echo "[proof] recv response: $RECV_RESULT"
    exit 1
fi

# Session B sends back
curl -sf -X POST "$API_URL/api/provider/peer/gossip_send" \
  -H "Authorization: Bearer $TOKEN_B" \
  -H "X-Capability-Token: $PEER_CAP_B" \
  -H "Content-Type: application/json" \
  -d '{"topic":"#general","message":"hello-from-B","sender":"bob","sender_id":"did:test:B","sender_session_id":"session-B","ts":1001}' > /dev/null

sleep 1
RECV_A=$(curl -sf -X POST "$API_URL/api/provider/peer/gossip_recv" \
  -H "Authorization: Bearer $TOKEN_A" \
  -H "X-Capability-Token: $PEER_CAP_A" \
  -H "Content-Type: application/json" \
  -d '{"topic":"#general","limit":50,"consumer_id":"consumer-A"}')

HAS_REPLY=$(echo "$RECV_A" | python3 -c "
import sys, json
data = json.load(sys.stdin)
messages = data.get('data', {}).get('messages', [])
for m in messages:
    if m.get('content') == 'hello-from-B':
        print('YES')
        break
else:
    print('NO')
")

if [[ "$HAS_REPLY" == "YES" ]]; then
    echo "[proof] A received B's message: PASS"
    echo ""
    echo "[shared-runtime-gossip-proof] PASS — bidirectional delivery proven"
else
    echo "[proof] A did NOT receive B's message: FAIL"
    echo "[proof] recv response: $RECV_A"
    exit 1
fi
