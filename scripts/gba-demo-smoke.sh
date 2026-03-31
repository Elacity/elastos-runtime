#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEMO_HOME="${ELASTOS_GBA_SMOKE_HOME:-$(mktemp -d /tmp/elastos-gba-smoke-XXXXXX)}"
KEEP_HOME="${ELASTOS_GBA_SMOKE_KEEP_HOME:-0}"
BUILD_ARGS=()

if [[ "${1:-}" == "--skip-build" ]]; then
    BUILD_ARGS+=(--skip-build)
fi

cleanup() {
    if [[ -n "${SERVER_PID:-}" ]]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    if [[ "$KEEP_HOME" != "1" ]]; then
        rm -rf "$DEMO_HOME"
    fi
}
trap cleanup EXIT

echo "[gba-demo] prepare clean temp-home demo"
bash "$ROOT/scripts/pc2-demo-local.sh" --prepare-only "${BUILD_ARGS[@]}" --home "$DEMO_HOME" >/tmp/elastos-gba-demo-prepare.log

ADDR="$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
host, port = s.getsockname()
print(f"{host}:{port}")
s.close()
PY
)"

LOG_PATH="$(mktemp /tmp/elastos-gba-serve-XXXXXX.log)"
echo "[gba-demo] serve gba-ucity on $ADDR"
(
    cd "$ROOT"
    HOME="$DEMO_HOME" \
    XDG_DATA_HOME="$DEMO_HOME/xdg-data" \
    ./elastos/target/debug/elastos serve --addr "$ADDR" --capsule "$ROOT/capsules/gba-ucity"
) >"$LOG_PATH" 2>&1 &
SERVER_PID="$!"

BASE_URL="http://$ADDR"

echo "[gba-demo] wait for viewer bootstrap"
python3 - "$BASE_URL" <<'PY'
import sys, time, urllib.request
base = sys.argv[1]
deadline = time.time() + 15
last_error = None
while time.time() < deadline:
    try:
        with urllib.request.urlopen(base + "/api/capsule/bootstrap", timeout=2) as resp:
            if resp.status == 200:
                sys.exit(0)
    except Exception as exc:
        last_error = exc
        time.sleep(0.2)
print(f"bootstrap not ready: {last_error}", file=sys.stderr)
sys.exit(1)
PY

echo "[gba-demo] verify headers and assets"
python3 - "$BASE_URL" <<'PY'
import json, sys, urllib.request
base = sys.argv[1]

def headers(path):
    with urllib.request.urlopen(base + path) as resp:
        return resp.headers, resp.read()

root_headers, root_body = headers("/")
assert root_headers.get("Cross-Origin-Opener-Policy") == "same-origin", root_headers
assert root_headers.get("Cross-Origin-Embedder-Policy") == "require-corp", root_headers
assert b"GBA Emulator - ElastOS" in root_body

_, mgba_js = headers("/mgba.js")
assert len(mgba_js) > 1000

_, mgba_wasm = headers("/mgba.wasm")
assert len(mgba_wasm) > 100000

_, boot_bytes = headers("/api/capsule/bootstrap")
boot = json.loads(boot_bytes)
assert boot["name"] == "gba-ucity", boot
assert boot["rom"] == "ucity.gba", boot

_, rom = headers(f"/capsule-data/{boot['rom']}?capsule={boot['name']}")
assert len(rom) > 100000, len(rom)
assert rom[:4] != b"", rom[:4]

print(json.dumps({
    "name": boot["name"],
    "rom": boot["rom"],
    "mgba_js_bytes": len(mgba_js),
    "mgba_wasm_bytes": len(mgba_wasm),
    "rom_bytes": len(rom),
}, indent=2))
PY

echo "[gba-demo] OK"
