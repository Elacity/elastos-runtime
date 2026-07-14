#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
STATE_DIR="$(mktemp -d)"
PROFILE_DIR="$(mktemp -d)"
SERVER_LOG="$STATE_DIR/server.log"
BROWSER_LOG="$STATE_DIR/browser.log"
INPUT_LOG="$STATE_DIR/input.log"
SERVER_PID=""
BROWSER_PID=""
INPUT_PID=""

cleanup() {
    [[ -n "$INPUT_PID" ]] && kill "$INPUT_PID" 2>/dev/null || true
    [[ -n "$INPUT_PID" ]] && wait "$INPUT_PID" 2>/dev/null || true
    [[ -n "$BROWSER_PID" ]] && kill "$BROWSER_PID" 2>/dev/null || true
    [[ -n "$BROWSER_PID" ]] && wait "$BROWSER_PID" 2>/dev/null || true
    pkill -TERM -f "$PROFILE_DIR/profile" 2>/dev/null || true
    [[ -n "$SERVER_PID" ]] && kill "$SERVER_PID" 2>/dev/null || true
    [[ -n "$SERVER_PID" ]] && wait "$SERVER_PID" 2>/dev/null || true
    for _ in $(seq 1 20); do
        rm -rf "$STATE_DIR" "$PROFILE_DIR" 2>/dev/null && return
        sleep 0.1
    done
    rm -rf "$STATE_DIR" "$PROFILE_DIR"
}
trap cleanup EXIT
mkdir -p "$PROFILE_DIR/profile"

chromium_bin="${ELASTOS_GBA_CHROMIUM_BIN:-}"
if [[ -z "$chromium_bin" ]]; then
    for candidate in /usr/bin/chromium /usr/bin/chromium-browser /usr/bin/google-chrome; do
        if [[ -x "$candidate" ]]; then
            chromium_bin="$candidate"
            break
        fi
    done
fi
if [[ -z "$chromium_bin" || ! -x "$chromium_bin" ]]; then
    echo "Chromium not found; set ELASTOS_GBA_CHROMIUM_BIN" >&2
    exit 2
fi

python3 "$ROOT/scripts/fixtures/gba-linux-browser-proof/server.py" "$ROOT" "$STATE_DIR" >"$SERVER_LOG" 2>&1 &
SERVER_PID="$!"

for _ in $(seq 1 50); do
    if curl -fsS http://127.0.0.1:8765/index.html >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done
curl -fsS http://127.0.0.1:8765/index.html >/dev/null

HOME="$PROFILE_DIR" "$chromium_bin" \
    --headless=new \
    --no-sandbox \
    --no-first-run \
    --no-default-browser-check \
    --autoplay-policy=no-user-gesture-required \
    --remote-debugging-port=9222 \
    --remote-allow-origins='*' \
    --user-data-dir="$PROFILE_DIR/profile" \
    'http://127.0.0.1:8765/?capsule=gba-ucity&home_token=linux-browser-proof' \
    >"$BROWSER_LOG" 2>&1 &
BROWSER_PID="$!"
node "$ROOT/scripts/fixtures/gba-linux-browser-proof/cdp-input.mjs" >"$INPUT_LOG" 2>&1 &
INPUT_PID="$!"

for _ in $(seq 1 600); do
    [[ -s "$STATE_DIR/result.json" ]] && break
    kill -0 "$BROWSER_PID" 2>/dev/null || {
        cat "$SERVER_LOG" >&2
        cat "$BROWSER_LOG" >&2
        cat "$INPUT_LOG" >&2 2>/dev/null || true
        echo "Linux browser exited before GBA proof completed" >&2
        exit 1
    }
    sleep 0.1
done

if [[ ! -s "$STATE_DIR/result.json" ]]; then
    cat "$SERVER_LOG" >&2
    cat "$BROWSER_LOG" >&2
    cat "$INPUT_LOG" >&2 2>/dev/null || true
    echo "GBA Linux browser proof timed out" >&2
    exit 1
fi

python3 - "$STATE_DIR/result.json" <<'PY'
import json
import pathlib
import sys

result = json.loads(pathlib.Path(sys.argv[1]).read_text())
required = {
    "ok": result.get("ok") is True,
    "crossOriginIsolated": result.get("crossOriginIsolated") is True,
    "sharedArrayBuffer": result.get("sharedArrayBuffer") is True,
    "reloaded": result.get("reloaded") is True,
    "renderedAfterReload": result.get("renderedAfterReload") is True,
    "save.put_count": result.get("save", {}).get("put_count", 0) > 0,
    "save.get_after_put": result.get("save", {}).get("get_after_put", 0) > 0,
    "save.save_bytes": result.get("save", {}).get("save_bytes", 0) > 0,
    "errors": not result.get("errors"),
}
failed = [name for name, passed in required.items() if not passed]
if failed:
    print(json.dumps(result, indent=2), file=sys.stderr)
    raise SystemExit("failed Linux GBA browser checks: " + ", ".join(failed))
print(
    "[gba-linux-browser] OK "
    f"platform={result.get('platform')} "
    f"save_bytes={result['save']['save_bytes']} "
    "render=ok input=ok audio=ok reload=ok cleanup=ephemeral"
)
PY
