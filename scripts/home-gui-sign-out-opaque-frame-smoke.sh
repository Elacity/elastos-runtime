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

python3 "$ROOT/scripts/fixtures/home-gui-sign-out-opaque-frame-proof/server.py" \
    "$ROOT" "$STATE_DIR" >"$SERVER_LOG" 2>&1 &
SERVER_PID="$!"

for _ in $(seq 1 50); do
    [[ -s "$STATE_DIR/server-port" ]] && break
    kill -0 "$SERVER_PID" 2>/dev/null || {
        cat "$SERVER_LOG" >&2
        echo "Home GUI sign-out proof server exited before binding" >&2
        exit 1
    }
    sleep 0.1
done
if [[ ! -s "$STATE_DIR/server-port" ]]; then
    cat "$SERVER_LOG" >&2
    echo "Home GUI sign-out proof server did not report its port" >&2
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
        echo "Chromium exited before Home GUI sign-out proof completed" >&2
        exit 1
    }
    sleep 0.1
done
if [[ ! -s "$STATE_DIR/result.json" ]]; then
    cat "$SERVER_LOG" >&2
    cat "$BROWSER_LOG" >&2
    echo "Home GUI sign-out opaque-frame proof timed out" >&2
    exit 1
fi

python3 - "$STATE_DIR/result.json" <<'PY'
import json
import pathlib
import sys

result = json.loads(pathlib.Path(sys.argv[1]).read_text())
required = {
    "result": result.get("ok") is True,
    "valid shell ready": result.get("valid_shell_ready") is True,
    "initial authority absent": result.get("initial", {}).get("authority") == "",
    "initial sign-out disabled": result.get("initial", {}).get("hidden") is True,
    "initial sign-out hidden": result.get("initial", {}).get("display") == "none",
    "forged source rejected": result.get("forged_source", {}).get("authority") == "",
    "forged source sign-out disabled": result.get("forged_source", {}).get("hidden") is True,
    "forged source stays hidden": result.get("forged_source", {}).get("display") == "none",
    "trusted signed projected": result.get("trusted_signed", {}).get("authority") == "signed",
    "trusted signed enables sign-out": result.get("trusted_signed", {}).get("hidden") is False,
    "trusted signed presentation enabled": result.get("trusted_signed", {}).get("display") == "flex",
    "trusted signed-out projected": result.get("trusted_signed_out", {}).get("authority") == "unsigned",
    "trusted signed-out disables sign-out": result.get("trusted_signed_out", {}).get("hidden") is True,
    "trusted signed-out hidden": result.get("trusted_signed_out", {}).get("display") == "none",
    "forged origin rejected": result.get("forged_origin", {}).get("authority") == "",
    "forged origin sign-out disabled": result.get("forged_origin", {}).get("hidden") is True,
    "forged origin stays hidden": result.get("forged_origin", {}).get("display") == "none",
    "one sign-out message": result.get("sign_out_messages") == 1,
}
failed = [name for name, passed in required.items() if not passed]
if failed:
    print(json.dumps(result, indent=2), file=sys.stderr)
    raise SystemExit("failed Home GUI sign-out checks: " + ", ".join(failed))
print(
    "[home-gui-sign-out-opaque-frame] PASS "
    f"checks={len(required)} source=checked origin=checked sign_out_messages=1"
)
PY
