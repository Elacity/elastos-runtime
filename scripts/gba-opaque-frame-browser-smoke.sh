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
BROWSER_PGID=""
INPUT_PID=""

cleanup() {
    [[ -n "$INPUT_PID" ]] && kill "$INPUT_PID" 2>/dev/null || true
    [[ -n "$INPUT_PID" ]] && wait "$INPUT_PID" 2>/dev/null || true
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
    echo "Chromium not found; set ELASTOS_GBA_CHROMIUM_BIN" >&2
    exit 2
fi
python3 "$ROOT/scripts/fixtures/gba-opaque-frame-browser-proof/server.py" "$ROOT" "$STATE_DIR" >"$SERVER_LOG" 2>&1 &
SERVER_PID="$!"

for _ in $(seq 1 50); do
    if [[ -s "$STATE_DIR/server-port" ]]; then
        break
    fi
    kill -0 "$SERVER_PID" 2>/dev/null || {
        cat "$SERVER_LOG" >&2
        echo "GBA proof server exited before binding" >&2
        exit 1
    }
    sleep 0.1
done
if [[ ! -s "$STATE_DIR/server-port" ]]; then
    cat "$SERVER_LOG" >&2
    echo "GBA proof server did not report its port" >&2
    exit 1
fi
SERVER_PORT="$(<"$STATE_DIR/server-port")"
curl -fsS --noproxy '*' --proto '=http' --max-redirs 0 "http://127.0.0.1:$SERVER_PORT/" >/dev/null

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
    --remote-debugging-port=0 \
    --remote-allow-origins='*' \
    --user-data-dir="$PROFILE_DIR/profile" \
    "http://127.0.0.1:$SERVER_PORT/" \
    >"$BROWSER_LOG" 2>&1 &
BROWSER_PID="$!"
BROWSER_PGID="$BROWSER_PID"
ELASTOS_GBA_PROFILE_DIR="$PROFILE_DIR/profile" \
ELASTOS_GBA_SERVER_PORT="$SERVER_PORT" \
    node "$ROOT/scripts/fixtures/gba-opaque-frame-browser-proof/cdp-input.mjs" >"$INPUT_LOG" 2>&1 &
INPUT_PID="$!"

for _ in $(seq 1 600); do
    [[ -s "$STATE_DIR/result.json" ]] && break
    kill -0 "$BROWSER_PID" 2>/dev/null || {
        cat "$SERVER_LOG" >&2
        cat "$BROWSER_LOG" >&2
        cat "$INPUT_LOG" >&2 2>/dev/null || true
        echo "Chromium exited before opaque-frame GBA proof completed" >&2
        exit 1
    }
    sleep 0.1
done

if [[ ! -s "$STATE_DIR/result.json" ]]; then
    cat "$SERVER_LOG" >&2
    cat "$BROWSER_LOG" >&2
    cat "$INPUT_LOG" >&2 2>/dev/null || true
    echo "GBA opaque-frame Chromium proof timed out" >&2
    exit 1
fi

if ! wait "$INPUT_PID"; then
    INPUT_PID=""
    cat "$INPUT_LOG" >&2
    echo "GBA trusted-input proof failed" >&2
    exit 1
fi
INPUT_PID=""

python3 - "$STATE_DIR/result.json" <<'PY'
import json
import pathlib
import sys

result = json.loads(pathlib.Path(sys.argv[1]).read_text())
required = {
    "ok": result.get("ok") is True,
    "platform": bool(str(result.get("platform", ""))),
    "crossOriginIsolated": result.get("crossOriginIsolated") is False,
    "sharedArrayBuffer": result.get("sharedArrayBuffer") is False,
    "opaque frame": result.get("topology", {}).get("message_origin") == "null",
    "sandbox excludes allow-same-origin": result.get("topology", {}).get("allows_same_origin") is False,
    "sandbox excludes popup escape": result.get("topology", {}).get("allows_popup_escape") is False,
    "credentialless is absent": result.get("topology", {}).get("credentialless") is False,
    "parent cannot read frame": result.get("topology", {}).get("parent_can_read_frame") is False,
    "negative control is readable": result.get("topology", {}).get("negative_control_readable") is True,
    "selected object": result.get("topology", {}).get("selected_resource") == "gba-ucity",
    "executable actor": result.get("topology", {}).get("executable_actor") == "gba-emulator",
    "opaque API origin": bool(result.get("topology", {}).get("api_origins")) and set(result["topology"]["api_origins"]) == {"null"},
    "trusted keydown": result.get("trusted_input", {}).get("keydown_trusted") is True,
    "trusted keyup": result.get("trusted_input", {}).get("keyup_trusted") is True,
    "trusted press mapping": result.get("trusted_input", {}).get("pressed") is True,
    "trusted release mapping": result.get("trusted_input", {}).get("released") is True,
    "trusted Start keydown": result.get("trusted_input", {}).get("start_keydown_trusted") is True,
    "trusted Start keyup": result.get("trusted_input", {}).get("start_keyup_trusted") is True,
    "trusted Start press mapping": result.get("trusted_input", {}).get("start_pressed") is True,
    "trusted Start release mapping": result.get("trusted_input", {}).get("start_released") is True,
    "initial nonblank render": result.get("initial", {}).get("rendered") is True,
    "initial render continuity": result.get("initial", {}).get("renderContinuity") is True,
    "initial canvas activity": sum(result.get("initial", {}).get("renderActivity", {}).get(name, 0) for name in ("put_image_data_during_observation", "draws_during_observation", "texture_uploads_during_observation")) > 0,
    "initial changing pixels": len(result.get("initial", {}).get("renderActivity", {}).get("distinct_pixel_hashes", [])) > 1 and (result.get("initial", {}).get("renderActivity", {}).get("changed_framebuffer_uploads", 0) > 0 or result.get("initial", {}).get("renderActivity", {}).get("changed_frame_writes", 0) > 0),
    "initial nonzero render data": result.get("initial", {}).get("renderActivity", {}).get("nonzero_framebuffer_bytes", 0) > 0 or result.get("initial", {}).get("renderActivity", {}).get("nonzero_image_data_bytes", 0) > 0,
    "initial audio callbacks": result.get("initial", {}).get("audioOutput", {}).get("script_processor_callbacks", 0) > 0,
    "initial nonzero audio": result.get("initial", {}).get("audioOutput", {}).get("nonzero_samples", 0) > 0 and result.get("initial", {}).get("audioOutput", {}).get("max_abs_sample", 0) > 0,
    "reloaded": result.get("reloaded") is True,
    "renderedAfterReload": result.get("renderedAfterReload") is True,
    "reload canvas activity": sum(result.get("renderActivityAfterReload", {}).get(name, 0) for name in ("put_image_data_during_observation", "draws_during_observation", "texture_uploads_during_observation")) > 0,
    "reload frame write": result.get("renderActivityAfterReload", {}).get("framebuffer_uploads", 0) > 0 or result.get("renderActivityAfterReload", {}).get("put_image_data_calls", 0) > 0,
    "reload nonzero render data": result.get("renderActivityAfterReload", {}).get("nonzero_framebuffer_bytes", 0) > 0 or result.get("renderActivityAfterReload", {}).get("nonzero_image_data_bytes", 0) > 0,
    "save.put_count": result.get("save", {}).get("put_count", 0) > 0,
    "save.get_after_put": result.get("save", {}).get("get_after_put", 0) > 0,
    "save.save_bytes": result.get("save", {}).get("save_bytes", 0) > 0,
    "save.state_put_count": result.get("save", {}).get("state_put_count", 0) > 0,
    "save.state_get_after_put": result.get("save", {}).get("state_get_after_put", 0) > 0,
    "save.state_bytes": result.get("save", {}).get("state_bytes", 0) > 0,
    "stateLoadedAfterReload": result.get("stateLoadedAfterReload") is True,
    "errors": not result.get("errors"),
}
failed = [name for name, passed in required.items() if not passed]
if failed:
    print(json.dumps(result, indent=2), file=sys.stderr)
    raise SystemExit("failed opaque-frame GBA browser checks: " + ", ".join(failed))
print(
    "[gba-opaque-frame-browser] OK "
    f"platform={result.get('platform')} "
    f"save_bytes={result['save']['save_bytes']} "
    f"state_bytes={result['save']['state_bytes']} "
    f"canvas_draws={result['initial']['renderActivity']['draws_during_observation']} "
    f"image_writes={result['initial']['renderActivity']['put_image_data_during_observation']} "
    f"texture_uploads={result['initial']['renderActivity']['texture_uploads_during_observation']} "
    f"pixel_hashes={len(result['initial']['renderActivity']['distinct_pixel_hashes'])} "
    f"audio_callbacks={result['initial']['audioOutput']['script_processor_callbacks']} "
    f"nonzero_audio_samples={result['initial']['audioOutput']['nonzero_samples']} "
    "topology=opaque-frame object=gba-ucity actor=gba-emulator "
    "render=pixel-changing input=trusted audio=nonzero-output save_state=ok reload=ok cleanup=ephemeral"
)
PY
