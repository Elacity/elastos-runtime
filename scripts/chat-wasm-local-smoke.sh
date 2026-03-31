#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_HOME="${ELASTOS_CHAT_WASM_SMOKE_HOME:-$(mktemp -d /tmp/elastos-chat-wasm-smoke-XXXXXX)}"
OUTPUT_LOG="${TEST_HOME}/chat-wasm.log"

cleanup() {
    rm -f "$OUTPUT_LOG"
}
trap cleanup EXIT

echo "[chat-wasm-local-smoke] home: ${TEST_HOME}"

if ! ROOT="$ROOT" TEST_HOME="$TEST_HOME" OUTPUT_LOG="$OUTPUT_LOG" \
    timeout 45 python3 - <<'PY'
import os
import pty
import select
import subprocess
import sys
import time
import errno

root = os.environ["ROOT"]
test_home = os.environ["TEST_HOME"]
output_log = os.environ["OUTPUT_LOG"]

master, slave = pty.openpty()
cmd = [
    "bash",
    "scripts/chat-wasm-local.sh",
    "--home",
    test_home,
    "--nick",
    "smoke",
]
proc = subprocess.Popen(
    cmd,
    cwd=root,
    stdin=slave,
    stdout=slave,
    stderr=slave,
    env=os.environ.copy(),
    close_fds=True,
)
os.close(slave)

captured = bytearray()
deadline = time.time() + 35
banner_seen = False
quit_sent = False

while time.time() < deadline:
    ready, _, _ = select.select([master], [], [], 0.25)
    if ready:
        try:
            chunk = os.read(master, 4096)
        except OSError as exc:
            if exc.errno == errno.EIO:
                break
            raise
        if not chunk:
            break
        captured.extend(chunk)
        text = captured.decode("utf-8", errors="ignore")
        if "ElastOS IRC v" in text and "smoke | #general" in text and not quit_sent:
            os.write(master, b"/quit\r")
            quit_sent = True
            banner_seen = True

    if quit_sent and proc.poll() is not None:
        break

try:
    if proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)
finally:
    os.close(master)
    with open(output_log, "wb") as fh:
        fh.write(captured)

if not banner_seen:
    sys.stderr.write("[chat-wasm-local-smoke] did not observe IRC banner before timeout\n")
    sys.exit(1)

if proc.returncode not in (0, None):
    sys.stderr.write(f"[chat-wasm-local-smoke] unexpected exit code: {proc.returncode}\n")
    sys.exit(1)
PY
then
    echo "[chat-wasm-local-smoke] launch failed. Output:" >&2
    cat "$OUTPUT_LOG" >&2
    exit 1
fi

grep -q "launch explicit WASM chat target" "$OUTPUT_LOG" \
    || { echo "[chat-wasm-local-smoke] missing launch marker" >&2; cat "$OUTPUT_LOG" >&2; exit 1; }
grep -q "ElastOS IRC v" "$OUTPUT_LOG" \
    || { echo "[chat-wasm-local-smoke] missing IRC banner" >&2; cat "$OUTPUT_LOG" >&2; exit 1; }
grep -q "smoke | #general" "$OUTPUT_LOG" \
    || { echo "[chat-wasm-local-smoke] missing explicit nick in chat banner" >&2; cat "$OUTPUT_LOG" >&2; exit 1; }

echo "[chat-wasm-local-smoke] pass"
