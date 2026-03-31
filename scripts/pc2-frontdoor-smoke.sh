#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOME_DIR="$(mktemp -d /tmp/elastos-pc2-frontdoor-XXXXXX)"
trap 'rm -rf "$HOME_DIR"' EXIT

PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"
MAINTAINER_DID="${ELASTOS_MAINTAINER_DID:-did:key:z6Mkf2nCJ1pcN4JioAxHEiyDsPC298QFtn2Dgg9tjt2ezHeK}"
SOURCE_PC2_DIR="$ROOT/capsules/pc2"
SOURCE_PC2_WASM="$SOURCE_PC2_DIR/target/wasm32-wasip1/release/pc2.wasm"
SOURCE_COMPONENTS_MANIFEST="$ROOT/components.json"
OPERATOR_HOME="${HOME}"

discover_source_bootstrap() {
    local coords_file="${OPERATOR_HOME}/.local/share/elastos/runtime-coords.json"
    if [[ ! -f "$coords_file" ]]; then
        echo "[pc2-frontdoor] runtime coords missing: $coords_file" >&2
        return 1
    fi

    RUNTIME_COORDS="$coords_file" python3 - <<'PY'
import json
import os
import urllib.request

coords = json.loads(open(os.environ["RUNTIME_COORDS"]).read())
api_url = coords["api_url"]
secret = coords["attach_secret"]

attach_req = urllib.request.Request(
    api_url + "/api/auth/attach",
    data=json.dumps({"secret": secret, "scope": "shell"}).encode("utf-8"),
    headers={"Content-Type": "application/json"},
)
with urllib.request.urlopen(attach_req, timeout=5) as resp:
    token = json.loads(resp.read().decode("utf-8"))["token"]

ticket_req = urllib.request.Request(
    api_url + "/api/provider/peer/get_ticket",
    data=b"{}",
    headers={
        "Content-Type": "application/json",
        "Authorization": f"Bearer {token}",
    },
)
with urllib.request.urlopen(ticket_req, timeout=5) as resp:
    body = json.loads(resp.read().decode("utf-8"))

print(body["data"]["ticket"])
print(body["data"]["node_id"])
PY
}

echo "[pc2-frontdoor] build elastos binary"
cargo build --manifest-path "$ROOT/elastos/Cargo.toml" -p elastos-server >/dev/null

echo "[pc2-frontdoor] build pc2 wasm"
cargo build --manifest-path "$ROOT/capsules/pc2/Cargo.toml" --target wasm32-wasip1 --release >/dev/null
cp "$SOURCE_PC2_WASM" "$SOURCE_PC2_DIR/pc2.wasm"

echo "[pc2-frontdoor] seed trusted source"
mapfile -t SOURCE_BOOTSTRAP < <(discover_source_bootstrap)
SOURCE_CONNECT_TICKET="${SOURCE_BOOTSTRAP[0]:-}"
SOURCE_NODE_ID="${SOURCE_BOOTSTRAP[1]:-}"
if [[ -z "$SOURCE_CONNECT_TICKET" || -z "$SOURCE_NODE_ID" ]]; then
    echo "[pc2-frontdoor] failed to discover live trusted-source Carrier bootstrap" >&2
    exit 1
fi
HOME="$HOME_DIR" \
XDG_DATA_HOME="$HOME_DIR/xdg-data" \
ELASTOS_PUBLISHER_GATEWAY="$PUBLISHER_GATEWAY" \
ELASTOS_MAINTAINER_DID="$MAINTAINER_DID" \
ELASTOS_SOURCE_CONNECT_TICKET="$SOURCE_CONNECT_TICKET" \
ELASTOS_PUBLISHER_NODE_ID="$SOURCE_NODE_ID" \
bash "$ROOT/scripts/install.sh" >/tmp/elastos-pc2-frontdoor-install.log

if [[ ! -f "$SOURCE_COMPONENTS_MANIFEST" ]]; then
    echo "[pc2-frontdoor] source components manifest missing: $SOURCE_COMPONENTS_MANIFEST" >&2
    exit 1
fi

echo "[pc2-frontdoor] setup pc2 profile"
cp "$SOURCE_COMPONENTS_MANIFEST" "$HOME_DIR/xdg-data/elastos/components.json"
HOME="$HOME_DIR" \
XDG_DATA_HOME="$HOME_DIR/xdg-data" \
"$ROOT/elastos/target/debug/elastos" setup --profile pc2 >/tmp/elastos-pc2-frontdoor-setup.log

INSTALLED_PC2_DIR="$HOME_DIR/xdg-data/elastos/capsules/pc2"
if [[ ! -d "$INSTALLED_PC2_DIR" ]]; then
    echo "[pc2-frontdoor] installed pc2 capsule missing after setup: $INSTALLED_PC2_DIR" >&2
    exit 1
fi
mv "$INSTALLED_PC2_DIR" "${INSTALLED_PC2_DIR}.installed"

echo "[pc2-frontdoor] prove current source elastos + current source pc2.wasm against clean-home data"
HOME_DIR="$HOME_DIR" ROOT="$ROOT" SOURCE_COMPONENTS_MANIFEST="$SOURCE_COMPONENTS_MANIFEST" python3 - <<'PY'
import os
import pty
import select
import signal
import subprocess
import sys
import time

home = os.environ["HOME_DIR"]
root = os.environ["ROOT"]
env = os.environ.copy()
env["HOME"] = home
env["XDG_DATA_HOME"] = f"{home}/xdg-data"
# Keep the smoke hermetic: chat launched from PC2 must stay on the slave PTY
# instead of probing the caller's controlling terminal via /dev/tty.
env["ELASTOS_CHAT_FORCE_STDIN"] = "1"
cmd = [f"{root}/elastos/target/debug/elastos"]

def run_case(label: str, payload: bytes) -> None:
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

    def read_for(seconds: float) -> str:
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

    def send(data: bytes, pause: float = 0.2) -> None:
        os.write(master, data)
        time.sleep(pause)

    try:
        initial = read_for(5.0)
        if "ElastOS PC2" not in initial:
            raise SystemExit(f"{label}: pc2 home did not render")

        send(b"\r\n")
        after_enter1 = read_for(3.0)
        if "Press Enter again to launch Chat" not in after_enter1:
            raise SystemExit(f"{label}: startup enter still launched or skipped without PC2 notice")

        send(b"\r", 0.8)
        after_enter2 = read_for(8.0)
        combined_chat = after_enter1 + after_enter2
        if "Connected to local runtime." not in combined_chat or "Chat as" not in combined_chat:
            raise SystemExit(f"{label}: second enter did not launch chat:\n{combined_chat}")
        send(payload, 0.5)
        after_exit = read_for(3.5)
        if "ElastOS PC2" not in after_exit:
            raise SystemExit(f"{label}: exit input did not return to PC2:\n{after_exit}")
    finally:
        if proc.poll() is None:
            os.killpg(proc.pid, signal.SIGTERM)
            try:
                proc.wait(timeout=2)
            except Exception:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait(timeout=2)
        os.close(master)

def run_navigation_case() -> None:
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

    def read_for(seconds: float) -> str:
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

    def send(data: bytes, pause: float = 0.2) -> None:
        os.write(master, data)
        time.sleep(pause)

    try:
        initial = read_for(5.0)
        if "\x1b[30;46;1m Home \x1b[0m" not in initial:
            raise SystemExit("nav: pc2 home did not start on Home")

        time.sleep(1.0)
        send(b"\x1b[C", 0.4)
        after_right = read_for(2.0)
        if "\x1b[30;46;1m People \x1b[0m" not in after_right:
            raise SystemExit(f"nav: right arrow did not switch to People:\n{after_right}")

        send(b"\t", 0.4)
        after_tab = read_for(2.0)
        if "\x1b[30;46;1m Spaces \x1b[0m" not in after_tab:
            raise SystemExit(f"nav: tab did not switch from People to Spaces:\n{after_tab}")
    finally:
        if proc.poll() is None:
            os.killpg(proc.pid, signal.SIGTERM)
            try:
                proc.wait(timeout=2)
            except Exception:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait(timeout=2)
        os.close(master)

def run_down_navigation_case() -> None:
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

    def read_for(seconds: float) -> str:
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

    def send(data: bytes, pause: float = 0.2) -> None:
        os.write(master, data)
        time.sleep(pause)

    try:
        initial = read_for(5.0)
        if "> 1 Chat [ready]" not in initial:
            raise SystemExit("nav-down: pc2 home did not highlight Chat first")

        time.sleep(1.0)
        send(b"\x1b[B", 0.4)
        after_down = read_for(2.0)
        if "> 2 MyWebSite [empty]" not in after_down:
            raise SystemExit(f"nav-down: down arrow did not move selection:\n{after_down}")
    finally:
        if proc.poll() is None:
            os.killpg(proc.pid, signal.SIGTERM)
            try:
                proc.wait(timeout=2)
            except Exception:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait(timeout=2)
        os.close(master)

run_navigation_case()
run_down_navigation_case()
run_case("esc", b"\x1b")
run_case("home", b"/home\r")
run_case("quit", b"/quit\r")

print("[pc2-frontdoor] OK")
PY
