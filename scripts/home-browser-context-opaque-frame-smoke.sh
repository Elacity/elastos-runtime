#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
STATE_DIR="$(mktemp -d)"
PROFILE_DIR="$(mktemp -d)"
SERVER_LOG="$STATE_DIR/server.log"
BROWSER_LOG="$STATE_DIR/browser.log"
SERVER_PID=""
BROWSER_PID=""
BROWSER_PGID=""

cleanup() {
    [[ -n "$BROWSER_PGID" ]] && kill -TERM -- "-$BROWSER_PGID" 2>/dev/null || true
    [[ -n "$BROWSER_PID" ]] && kill "$BROWSER_PID" 2>/dev/null || true
    [[ -n "$BROWSER_PID" ]] && wait "$BROWSER_PID" 2>/dev/null || true
    if [[ -n "$BROWSER_PGID" ]]; then
        for _ in $(seq 1 20); do
            kill -0 -- "-$BROWSER_PGID" 2>/dev/null || break
            sleep 0.1
        done
        kill -KILL -- "-$BROWSER_PGID" 2>/dev/null || true
    fi
    [[ -n "$SERVER_PID" ]] && kill "$SERVER_PID" 2>/dev/null || true
    [[ -n "$SERVER_PID" ]] && wait "$SERVER_PID" 2>/dev/null || true
    rm -rf "$STATE_DIR" "$PROFILE_DIR"
}
trap cleanup EXIT
mkdir -p "$PROFILE_DIR/profile"

chromium_bin="${ELASTOS_HOME_CHROMIUM_BIN:-}"
if [[ -z "$chromium_bin" ]]; then
    for candidate in \
        "$HOME"/Library/Caches/ms-playwright/chromium-*/chrome-mac-*/"Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing" \
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser" \
        /usr/bin/chromium \
        /usr/bin/chromium-browser \
        /usr/bin/google-chrome; do
        if [[ -x "$candidate" ]]; then
            chromium_bin="$candidate"
            break
        fi
    done
fi
if [[ -z "$chromium_bin" || ! -x "$chromium_bin" ]]; then
    echo "Chromium not found; set ELASTOS_HOME_CHROMIUM_BIN" >&2
    exit 2
fi

python3 "$ROOT/scripts/fixtures/home-browser-context-opaque-frame-proof/server.py" \
    "$ROOT" "$STATE_DIR" >"$SERVER_LOG" 2>&1 &
SERVER_PID="$!"

for _ in $(seq 1 50); do
    [[ -s "$STATE_DIR/server-port" ]] && break
    kill -0 "$SERVER_PID" 2>/dev/null || {
        cat "$SERVER_LOG" >&2
        echo "Home context proof server exited before binding" >&2
        exit 1
    }
    sleep 0.1
done
if [[ ! -s "$STATE_DIR/server-port" ]]; then
    cat "$SERVER_LOG" >&2
    echo "Home context proof server did not report its port" >&2
    exit 1
fi
SERVER_PORT="$(<"$STATE_DIR/server-port")"
curl -fsS --noproxy '*' --proto '=http' --max-redirs 0 \
    "http://127.0.0.1:$SERVER_PORT/" >/dev/null

python3 -c \
    'import os, sys; os.setsid(); os.execv(sys.argv[1], sys.argv[1:])' \
    "$chromium_bin" \
    --headless=new \
    --no-first-run \
    --no-default-browser-check \
    --disable-background-networking \
    --no-proxy-server \
    --disable-breakpad \
    --disable-component-update \
    --disable-domain-reliability \
    --disable-extensions \
    --disable-sync \
    --host-resolver-rules='MAP * ~NOTFOUND, EXCLUDE 127.0.0.1, EXCLUDE localhost' \
    --metrics-recording-only \
    --user-data-dir="$PROFILE_DIR/profile" \
    "http://127.0.0.1:$SERVER_PORT/" \
    >"$BROWSER_LOG" 2>&1 &
BROWSER_PID="$!"
BROWSER_PGID="$BROWSER_PID"

for _ in $(seq 1 300); do
    [[ -s "$STATE_DIR/result.json" ]] && break
    kill -0 "$BROWSER_PID" 2>/dev/null || {
        cat "$SERVER_LOG" >&2
        cat "$BROWSER_LOG" >&2
        echo "Chromium exited before Home context proof completed" >&2
        exit 1
    }
    sleep 0.1
done
if [[ ! -s "$STATE_DIR/result.json" ]]; then
    cat "$SERVER_LOG" >&2
    cat "$BROWSER_LOG" >&2
    echo "Home opaque-frame context proof timed out" >&2
    exit 1
fi

python3 - "$STATE_DIR/result.json" <<'PY'
import json
import pathlib
import sys

result = json.loads(pathlib.Path(sys.argv[1]).read_text())
first = result.get("first", {})
first_child = first.get("child", {})
second = result.get("second", {})
child = second.get("child", {})
required = {
    "result": result.get("ok") is True,
    "platform": bool(str(result.get("platform", ""))),
    "host context shape": second.get("host_context_valid") is True,
    "host storage": second.get("host_storage_context") == second.get("host_context"),
    "profile reload": result.get("same_top_level_profile_context") is True,
    "new opaque child": result.get("new_opaque_child") is True,
    "first opaque child storage": first_child.get("local_storage_unavailable") is True,
    "second opaque child storage": child.get("local_storage_unavailable") is True,
    "first child starts unbound": first_child.get("initial_context") == "",
    "second child starts unbound": child.get("initial_context") == "",
    "first child accepted host": first_child.get("accepted") is True,
    "second child accepted host": child.get("accepted") is True,
    "first child context matches host": first_child.get("accepted_context") == first.get("host_context"),
    "second child context matches host": child.get("accepted_context") == second.get("host_context"),
    "first invalid ready rejected": first.get("rejected_ready_count", 0) > 0,
    "second invalid ready rejected": second.get("rejected_ready_count", 0) > 0,
    "first valid ready accepted": first.get("accepted_ready_count") == 1,
    "second valid ready accepted": second.get("accepted_ready_count") == 1,
    "first no early context": first_child.get("context_messages_before_accepted_ready") == 0,
    "second no early context": child.get("context_messages_before_accepted_ready") == 0,
}
failed = [name for name, passed in required.items() if not passed]
if failed:
    print(json.dumps(result, indent=2), file=sys.stderr)
    raise SystemExit("failed Home browser-context checks: " + ", ".join(failed))
print(
    "[home-browser-context-opaque-frame] PASS "
    f"checks={len(required)} platform={result.get('platform')} "
    f"context={second.get('host_context')} reload=retained "
    "child=opaque-unbound-before-checked-handoff"
)
PY
