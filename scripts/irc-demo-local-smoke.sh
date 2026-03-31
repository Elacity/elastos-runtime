#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_HOME="${ELASTOS_IRC_LOCAL_SMOKE_HOME:-$(mktemp -d /tmp/elastos-irc-local-smoke-XXXXXX)}"
OUTPUT_LOG="${TEST_HOME}/irc-demo.log"
SKIP_BUILD=0

usage() {
    cat <<'EOF'
Usage:
  bash scripts/irc-demo-local-smoke.sh
  bash scripts/irc-demo-local-smoke.sh --skip-build

What it proves:
  1. Runs the source-local packaged IRC microVM path on a KVM-capable host
  2. Observes the full-screen IRC banner in a real PTY
  3. Exits cleanly via /quit

This is the source/KVM proof for WSL or another KVM-capable Linux host.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [[ ! -e /dev/kvm ]]; then
    echo "[irc-demo-local-smoke] /dev/kvm is not available on this host." >&2
    echo "[irc-demo-local-smoke] Run this smoke on WSL or another KVM-capable Linux host." >&2
    exit 2
fi

cleanup() {
    rm -f "$OUTPUT_LOG"
}
trap cleanup EXIT

echo "[irc-demo-local-smoke] home: ${TEST_HOME}"

if ! ROOT="$ROOT" TEST_HOME="$TEST_HOME" OUTPUT_LOG="$OUTPUT_LOG" SKIP_BUILD="$SKIP_BUILD" \
    timeout 240 python3 - <<'PY'
import errno
import os
import pty
import select
import subprocess
import sys
import time

root = os.environ["ROOT"]
test_home = os.environ["TEST_HOME"]
output_log = os.environ["OUTPUT_LOG"]
skip_build = os.environ["SKIP_BUILD"] == "1"

cmd = [
    "bash",
    "scripts/irc-demo-local.sh",
    "--home",
    test_home,
    "--nick",
    "smoke",
]
if skip_build:
    cmd.insert(2, "--skip-build")

master, slave = pty.openpty()
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
deadline = time.time() + 210
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
    sys.stderr.write("[irc-demo-local-smoke] did not observe IRC banner before timeout\n")
    sys.exit(1)

if proc.returncode not in (0, None):
    sys.stderr.write(f"[irc-demo-local-smoke] unexpected exit code: {proc.returncode}\n")
    sys.exit(1)
PY
then
    echo "[irc-demo-local-smoke] launch failed. Output:" >&2
    cat "$OUTPUT_LOG" >&2
    exit 1
fi

grep -q "launch IRC microVM chat" "$OUTPUT_LOG" \
    || { echo "[irc-demo-local-smoke] missing microVM launch marker" >&2; cat "$OUTPUT_LOG" >&2; exit 1; }
grep -q "ElastOS IRC v" "$OUTPUT_LOG" \
    || { echo "[irc-demo-local-smoke] missing IRC banner" >&2; cat "$OUTPUT_LOG" >&2; exit 1; }
grep -q "smoke | #general" "$OUTPUT_LOG" \
    || { echo "[irc-demo-local-smoke] missing explicit nick in IRC banner" >&2; cat "$OUTPUT_LOG" >&2; exit 1; }

echo "[irc-demo-local-smoke] pass"
