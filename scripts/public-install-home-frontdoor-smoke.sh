#!/usr/bin/env bash
set -euo pipefail

# This fixture owns setup, launch, and cleanup after bootstrap.
export ELASTOS_INSTALL_ONLY=1

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/lib/runtime-cleanup.sh"
source "${SCRIPT_DIR}/lib/public-install-guards.sh"

PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"
FORCE_RELAY_ONLY="${ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY:-0}"
HOME_DIR="$(smoke_home_dir public-home)"
DATA_DIR=""

cleanup() {
    if [[ -n "$DATA_DIR" ]] && ! cleanup_elastos_runtime_home "$HOME_DIR" "$DATA_DIR" "${RUN_BIN:-${HOME_DIR}/.local/bin/elastos}"; then
        echo "[public-home-frontdoor] cleanup could not verify shutdown; preserved ${HOME_DIR}" >&2
        exit 1
    fi
    rm -rf "$HOME_DIR"
}
trap cleanup EXIT
DATA_DIR="$(elastos_runtime_data_dir "$HOME_DIR" "${HOME_DIR}/xdg-data")"

echo "[public-home-frontdoor] install from public gateway"
HOME="$HOME_DIR" \
XDG_DATA_HOME="$HOME_DIR/xdg-data" \
ELASTOS_PUBLISHER_GATEWAY="$PUBLISHER_GATEWAY" \
bash --noprofile --norc -c 'set -eo pipefail; curl -fsSL "${ELASTOS_PUBLISHER_GATEWAY%/}/install.sh" | bash' \
    >"${HOME_DIR}/install.log"

INSTALLED_BIN="$HOME_DIR/.local/bin/elastos"
RUN_BIN="${ELASTOS_BIN_OVERRIDE:-$INSTALLED_BIN}"
SOURCES_PATH="$DATA_DIR/sources.json"
INSTALLED_COMPONENTS_MANIFEST="$DATA_DIR/components.json"
if [[ ! -x "$INSTALLED_BIN" ]]; then
    echo "[public-home-frontdoor] installed binary missing: $INSTALLED_BIN" >&2
    exit 1
fi
if [[ ! -x "$RUN_BIN" ]]; then
    echo "[public-home-frontdoor] run binary missing: $RUN_BIN" >&2
    exit 1
fi

echo "[public-home-frontdoor] prove stamped trusted source"
SOURCE_OUTPUT="$(
    HOME="$HOME_DIR" \
    XDG_DATA_HOME="$HOME_DIR/xdg-data" \
    "$INSTALLED_BIN" source show
)"
echo "$SOURCE_OUTPUT"
if ! grep -q "Bootstrap: peer ticket configured" <<<"$SOURCE_OUTPUT"; then
    echo "[public-home-frontdoor] expected stamped Carrier bootstrap ticket missing from source show" >&2
    exit 1
fi
if grep -q "Node ID:   none" <<<"$SOURCE_OUTPUT"; then
    echo "[public-home-frontdoor] expected stamped publisher node id missing from source show" >&2
    exit 1
fi

if [[ "$FORCE_RELAY_ONLY" == "1" ]]; then
    echo "[public-home-frontdoor] remove gateway override and direct addrs to force relay-only Carrier setup"
    SOURCES_PATH="$SOURCES_PATH" python3 - <<'PY'
import json
import os
import pathlib
import base64

path = pathlib.Path(os.environ["SOURCES_PATH"])
data = json.loads(path.read_text())
for source in data.get("sources", []):
    source["gateways"] = []
    ticket = source.get("connect_ticket", "")
    if ticket:
        pad = "=" * ((8 - len(ticket) % 8) % 8)
        decoded = json.loads(base64.b32decode(ticket.upper() + pad))
        for endpoint in decoded.get("endpoints", []):
            endpoint["addrs"] = [addr for addr in endpoint.get("addrs", []) if "Relay" in addr]
        source["connect_ticket"] = (
            base64.b32encode(json.dumps(decoded, separators=(",", ":")).encode())
            .decode()
            .lower()
            .rstrip("=")
        )
path.write_text(json.dumps(data, indent=2) + "\n")
PY
else
    echo "[public-home-frontdoor] use stamped trusted-source transports"
fi

echo "[public-home-frontdoor] setup default Home profile"
guard_branch_binary_requires_checksummed_public_manifest "$INSTALLED_COMPONENTS_MANIFEST" "[public-home-frontdoor]"
HOME="$HOME_DIR" \
XDG_DATA_HOME="$HOME_DIR/xdg-data" \
ELASTOS_COMPONENTS_MANIFEST="$INSTALLED_COMPONENTS_MANIFEST" \
"$RUN_BIN" setup >"${HOME_DIR}/setup.log"

echo "[public-home-frontdoor] prove installed elastos -> home -> chat -> home/quit/esc -> home"
HOME_DIR="$HOME_DIR" RUN_BIN="$RUN_BIN" ELASTOS_COMPONENTS_MANIFEST="$INSTALLED_COMPONENTS_MANIFEST" python3 - <<'PY'
import os
import pty
import select
import signal
import subprocess
import time

home = os.environ["HOME_DIR"]
run_bin = os.environ["RUN_BIN"]
env = os.environ.copy()
env["HOME"] = home
env["XDG_DATA_HOME"] = f"{home}/xdg-data"
# Keep the smoke hermetic: chat launched from Home must stay on the slave PTY
# instead of probing the caller's controlling terminal via /dev/tty.
env["ELASTOS_CHAT_FORCE_STDIN"] = "1"
cmd = [run_bin]
def launch_pty():
    master, slave = pty.openpty()
    proc = subprocess.Popen(
        cmd,
        stdin=slave,
        stdout=slave,
        stderr=slave,
        env=env,
        close_fds=True,
        start_new_session=True,
    )
    os.close(slave)
    return master, proc

def read_for(master: int, seconds: float) -> str:
    end = time.time() + seconds
    chunks: list[bytes] = []
    while time.time() < end:
        ready, _, _ = select.select([master], [], [], 0.1)
        if not ready:
            continue
        try:
            data = os.read(master, 65536)
        except OSError:
            break
        if not data:
            break
        chunks.append(data)
    return b"".join(chunks).decode("utf-8", "replace")

def read_until(label: str, master: int, predicate, timeout: float) -> str:
    end = time.time() + timeout
    combined = ""
    while time.time() < end:
        combined += read_for(master, 0.5)
        if predicate(combined):
            return combined
    raise SystemExit(f"{label}: timed out waiting for expected output:\n{combined}")

def send(master: int, data: bytes, pause: float = 0.2) -> None:
    os.write(master, data)
    time.sleep(pause)

def shutdown(master: int, proc: subprocess.Popen) -> None:
    try:
        if proc.poll() is None:
            os.killpg(proc.pid, signal.SIGTERM)
            try:
                proc.wait(timeout=2)
            except Exception:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait(timeout=2)
    finally:
        os.close(master)

def run_repeated_action_case() -> None:
    # Two actions in a row must keep Home open (the second one used to quit
    # with "provider localhost/delete failed [403 Forbidden]").
    master, proc = launch_pty()
    try:
        read_until("repeat", master, lambda text: "ElastOS Home" in text, 10.0)
        for attempt in (1, 2):
            send(master, b"r", 0.5)
            screen = read_until("repeat", master, lambda text: "ElastOS Home" in text, 8.0)
            if proc.poll() is not None or "ElastOS Home" not in screen or "Error:" in screen:
                raise SystemExit(f"repeat: installed Home did not stay open after action {attempt}:\n{screen}")
    finally:
        shutdown(master, proc)

def run_navigation_case() -> None:
    master, proc = launch_pty()
    try:
        initial = read_until(
            "nav",
            master,
            lambda text: "\x1b[30;46;1m Home \x1b[0m" in text,
            10.0,
        )
        if "\x1b[30;46;1m Home \x1b[0m" not in initial:
            raise SystemExit(f"nav: installed Home did not start on Home:\n{initial}")
        send(master, b"\x1b[C", 0.4)
        after_right = read_until(
            "nav",
            master,
            lambda text: "\x1b[30;46;1m Inbox \x1b[0m" in text,
            4.0,
        )
        if "\x1b[30;46;1m Inbox \x1b[0m" not in after_right:
            raise SystemExit(f"nav: installed right arrow did not switch to Inbox:\n{after_right}")
        send(master, b"\t", 0.4)
        after_tab = read_until(
            "nav",
            master,
            lambda text: "\x1b[30;46;1m People \x1b[0m" in text,
            4.0,
        )
        if "\x1b[30;46;1m People \x1b[0m" not in after_tab:
            raise SystemExit(f"nav: installed tab did not switch from Inbox to People:\n{after_tab}")
    finally:
        shutdown(master, proc)

run_navigation_case()
run_repeated_action_case()
print("[public-home-frontdoor] OK")
PY
