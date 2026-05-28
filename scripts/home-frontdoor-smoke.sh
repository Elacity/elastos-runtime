#!/usr/bin/env bash
#
# Phase 5 Day 2 — Mac port. Pulls in the cross-platform helper
# library to replace bash-4-only `mapfile` and the inline
# `free_port` / `host_platform` blocks with their shared
# audited equivalents. Adds:
#
#   - ELASTOS_VZ_SMOKE_DRY_RUN=1   early exit (CI fast lane)
#   - Mac pre-flight                graceful skip + Phase-6 msg
#   - Vz substrate probe at tail    diagnostic for Day-3+ work
#
# Linux behaviour is byte-identical (helper functions match
# the original inline implementations; mapfile → read loop is
# semantically equivalent on Linux). The Phase 5 Day 1 audit
# of these helpers (`bash -n`, unit tests, manual smoke) is
# the safety belt; see `docs/vz-backend/PHASE_5_DAY_1_NOTES.md`.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# shellcheck source=lib/cross-platform.sh
. "${ROOT}/scripts/lib/cross-platform.sh"

OS_TOKEN="$(uname -s | tr '[:upper:]' '[:lower:]')"

HOME_DIR="$(mktemp -d /tmp/elastos-home-frontdoor-XXXXXX)"
PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"
MAINTAINER_DID="${ELASTOS_MAINTAINER_DID:-did:key:z6MkrFPDgDi98Ek6AFHM3VT9bVJytnDf5mfHAV6gyrD5frYj}"
SOURCE_HOME_CLI_DIR="$ROOT/capsules/home-cli"
SOURCE_HOME_CLI_WASM="$SOURCE_HOME_CLI_DIR/target/wasm32-wasip1/release/home-cli.wasm"
SOURCE_HOME_DIR="$ROOT/capsules/home"
SOURCE_SYSTEM_DIR="$ROOT/capsules/system"
SOURCE_DOCUMENTS_DIR="$ROOT/capsules/documents"
SOURCE_LIBRARY_DIR="$ROOT/capsules/library"
SOURCE_INBOX_DIR="$ROOT/capsules/inbox"
SOURCE_COMPONENTS_MANIFEST="$ROOT/components.json"
SOURCE_RUNTIME_HOME="${HOME_DIR}/source-runtime"
SOURCE_RUNTIME_XDG="${SOURCE_RUNTIME_HOME}/xdg-data"
SOURCE_RUNTIME_DATA_DIR="${SOURCE_RUNTIME_XDG}/elastos"
SOURCE_RUNTIME_LOG="${HOME_DIR}/source-runtime.log"
SOURCE_RUNTIME_COORDS="${SOURCE_RUNTIME_DATA_DIR}/runtime-coords.json"

cleanup() {
    if [[ -n "${SOURCE_RUNTIME_PID:-}" ]] && pid_is_running "${SOURCE_RUNTIME_PID}"; then
        # **Phase 5 Day 2** — was `kill PID; wait PID`. The new
        # helper hands SIGTERM to both the group and the bare
        # PID, then escalates to SIGKILL after a 2 s grace, so
        # daemonised children that escaped the group still die.
        kill_pid_then_group "${SOURCE_RUNTIME_PID}" 2
        wait "${SOURCE_RUNTIME_PID}" 2>/dev/null || true
    fi
    # **Phase 5 Day 2** — was `mapfile -t temp_pids < <(...)`,
    # which only exists in bash 4+. Use a `while read` loop
    # (the same pattern Day-1's `local-carrier-setup-smoke.sh`
    # uses in `kill_temp_processes`) so we don't trip on
    # bash-3.2's behaviour of treating an empty `"${arr[@]}"`
    # as an unbound variable under `set -u`.
    while IFS= read -r pid; do
        [[ -z "$pid" || "$pid" == "$$" ]] && continue
        kill "$pid" 2>/dev/null || true
    done < <(pgrep -f "$HOME_DIR" || true)
    sleep 0.2
    while IFS= read -r pid; do
        [[ -z "$pid" || "$pid" == "$$" ]] && continue
        if pid_is_running "$pid"; then
            kill -9 "$pid" 2>/dev/null || true
        fi
    done < <(pgrep -f "$HOME_DIR" || true)
    rm -rf "$HOME_DIR"
    return 0
}
trap cleanup EXIT

# **Phase 5 Day 2** — was an inline `python3 - <<PY2 …`. The
# cross-platform helper is the same implementation, hoisted so
# future smokes can DRY.
free_port() {
    free_port_via_python3
}

discover_source_bootstrap() {
    local coords_file="$1"
    if [[ ! -f "$coords_file" ]]; then
        echo "[home-frontdoor] runtime coords missing: $coords_file" >&2
        return 1
    fi

    RUNTIME_COORDS="$coords_file" python3 - <<'PY2'
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
PY2
}

host_platform() {
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64)
            printf '%s\n' "linux-amd64"
            ;;
        Linux-aarch64|Linux-arm64)
            printf '%s\n' "linux-arm64"
            ;;
        # **Phase 5 Day 2** — Mac host support. The Carrier
        # install pipeline still requires the corresponding
        # `darwin-arm64` entry in components.json (Phase 6
        # deliverable); the pre-flight check before any cargo
        # builds is what surfaces the missing entry. On a
        # post-Phase-6 host this case lets the smoke proceed.
        Darwin-arm64)
            printf '%s\n' "darwin-arm64"
            ;;
        Darwin-x86_64)
            printf '%s\n' "darwin-amd64"
            ;;
        *)
            echo "[home-frontdoor] unsupported host platform: $(uname -s)-$(uname -m)" >&2
            exit 1
            ;;
    esac
}

# Phase 5 Day 8 — Day-5 + Day-6 inline precedence blocks
# consolidated into `cross_platform_smoke_should_dry_run` +
# `cross_platform_smoke_log_dry_run_reason`. Echo lines
# preserved byte-for-byte from the inline originals.
if [[ "${ELASTOS_VZ_SMOKE_FORCE_FULL:-0}" == "1" ]]; then
    echo "[home-frontdoor] FORCE_FULL=1 — forcing full smoke run (overrides CI auto-detect)"
fi

if cross_platform_smoke_should_dry_run; then
    cross_platform_smoke_log_dry_run_reason "[home-frontdoor]"
    echo "[home-frontdoor] dry-run mode: parse OK, helper sourced OK; exiting before cargo build"
    if [[ "${OS_TOKEN}" == "darwin" ]]; then
        if vz_host_is_capable; then
            echo "[home-frontdoor] dry-run: Vz host capability check passed (macOS 12+)"
        else
            echo "[home-frontdoor] dry-run: Vz host capability check skipped (not macOS 12+ or sw_vers unavailable)"
        fi
    fi
    exit 0
fi

# **Phase 5 Day 2** — Mac pre-flight, hoisted from Day 1's
# `local-carrier-setup-smoke.sh` into the shared
# `cross_platform_assert_native_binary_release_metadata`
# helper. The home-frontdoor smoke covers the same Carrier
# install half as Day 1's smoke plus first-party browser
# capsules; the same darwin-arm64 entries are required.
#
# Pre-Work removed the dishonest darwin entries; Phase 6
# restores truthful ones (per `docs/vz-backend/PLAN.md` L321).
# Between those, on Mac, this smoke cannot exercise the
# native-binary install path end-to-end. We exit 0 with a
# clear operator-facing message + skip telemetry.
#
# Required names match the script's downstream `mapping = {…}`
# (line 158-162 in the Python staging block): shell,
# localhost-provider, did-provider, webspace-provider.
if [[ "${OS_TOKEN}" == "darwin" ]] \
        && [[ "${ELASTOS_VZ_SMOKE_FORCE_PROCEED:-0}" != "1" ]]; then
    if ! cross_platform_assert_native_binary_release_metadata \
            "${SOURCE_COMPONENTS_MANIFEST}" \
            shell localhost-provider did-provider webspace-provider 2>/dev/null
    then
        cross_platform_print_phase6_skip_message
        echo "[home-frontdoor] Mac pre-flight: SKIP (Phase 6 prerequisite not met)"
        # Exit 0 — clean skip, not a smoke failure. CI dashboards
        # alert on the skip telemetry separately.
        exit 0
    fi
fi

echo "[home-frontdoor] build elastos binary"
cargo build --manifest-path "$ROOT/elastos/Cargo.toml" -p elastos-server >/dev/null

echo "[home-frontdoor] build first-party setup artifacts"
cargo build --manifest-path "$ROOT/elastos/capsules/shell/Cargo.toml" --release >/dev/null
cargo build --manifest-path "$ROOT/elastos/capsules/localhost-provider/Cargo.toml" --release >/dev/null
cargo build --manifest-path "$ROOT/capsules/did-provider/Cargo.toml" --release >/dev/null
cargo build --manifest-path "$ROOT/capsules/webspace-provider/Cargo.toml" --release >/dev/null

echo "[home-frontdoor] build Home CLI wasm"
cargo build --manifest-path "$ROOT/capsules/home-cli/Cargo.toml" --target wasm32-wasip1 --release >/dev/null
cp "$SOURCE_HOME_CLI_WASM" "$SOURCE_HOME_CLI_DIR/home-cli.wasm"

echo "[home-frontdoor] build Home and System browser capsule wasm"
cargo build --manifest-path "$ROOT/capsules/home/Cargo.toml" --target wasm32-wasip1 --release >/dev/null
cargo build --manifest-path "$ROOT/capsules/system/Cargo.toml" --target wasm32-wasip1 --release >/dev/null

echo "[home-frontdoor] stage source-local trusted source"
SOURCE_API_PORT="$(free_port)"
SETUP_PLATFORM="$(host_platform)"
mkdir -p "$SOURCE_RUNTIME_DATA_DIR/bin"
install -m 755     "$ROOT/elastos/target/release/localhost-provider"     "$SOURCE_RUNTIME_DATA_DIR/bin/localhost-provider"

COMPONENTS_SRC="$SOURCE_COMPONENTS_MANIFEST" COMPONENTS_DEST="$SOURCE_RUNTIME_DATA_DIR/components.json" DATA_DIR="$SOURCE_RUNTIME_DATA_DIR" PUBLISHER_ROOT="$SOURCE_RUNTIME_DATA_DIR/ElastOS/SystemServices/Publisher" SETUP_PLATFORM="$SETUP_PLATFORM" SHELL_BIN="$ROOT/elastos/target/release/shell" LOCALHOST_PROVIDER_BIN="$ROOT/elastos/target/release/localhost-provider" DID_PROVIDER_BIN="$ROOT/capsules/did-provider/target/release/did-provider" WEBSPACE_PROVIDER_BIN="$ROOT/capsules/webspace-provider/target/release/webspace-provider" HOME_CLI_DIR="$SOURCE_HOME_CLI_DIR" HOME_CAPSULE_DIR="$SOURCE_HOME_DIR" SYSTEM_CAPSULE_DIR="$SOURCE_SYSTEM_DIR" DOCUMENTS_CAPSULE_DIR="$SOURCE_DOCUMENTS_DIR" LIBRARY_CAPSULE_DIR="$SOURCE_LIBRARY_DIR" INBOX_CAPSULE_DIR="$SOURCE_INBOX_DIR" python3 - <<'PY2'
import hashlib
import json
import os
import pathlib
import shutil
import tarfile

components_src = pathlib.Path(os.environ["COMPONENTS_SRC"])
components_dest = pathlib.Path(os.environ["COMPONENTS_DEST"])
publisher_root = pathlib.Path(os.environ["PUBLISHER_ROOT"])
artifacts_dir = publisher_root / "artifacts"
artifacts_dir.mkdir(parents=True, exist_ok=True)
platform = os.environ["SETUP_PLATFORM"]

manifest = json.loads(components_src.read_text())

def platform_info(name):
    platforms = manifest["external"][name].get("platforms") or {}
    info = platforms.get(platform) or platforms.get("*")
    if not info:
        raise SystemExit(f"{name} missing release metadata for {platform}")
    return info

mapping = {
    "shell": pathlib.Path(os.environ["SHELL_BIN"]),
    "localhost-provider": pathlib.Path(os.environ["LOCALHOST_PROVIDER_BIN"]),
    "did-provider": pathlib.Path(os.environ["DID_PROVIDER_BIN"]),
    "webspace-provider": pathlib.Path(os.environ["WEBSPACE_PROVIDER_BIN"]),
}

for name, src in mapping.items():
    if not src.is_file():
        raise SystemExit(f"missing built artifact for {name}: {src}")
    info = platform_info(name)
    release_path = info.get("release_path")
    if not release_path:
        raise SystemExit(f"{name} missing release_path for {platform}")
    dest = artifacts_dir / release_path
    shutil.copy2(src, dest)
    data = dest.read_bytes()
    info["checksum"] = "sha256:" + hashlib.sha256(data).hexdigest()
    info["size"] = len(data)

home_cli_dir = pathlib.Path(os.environ["HOME_CLI_DIR"])
home_cli_manifest = platform_info("home-cli")
home_cli_release_path = home_cli_manifest.get("release_path")
if not home_cli_release_path:
    raise SystemExit(f"home-cli missing release_path for {platform}")
home_cli_archive = artifacts_dir / home_cli_release_path
with tarfile.open(home_cli_archive, "w:gz") as tar:
    tar.add(home_cli_dir / "capsule.json", arcname="home-cli/capsule.json")
    tar.add(
        home_cli_dir / "target/wasm32-wasip1/release/home-cli.wasm",
        arcname="home-cli/home-cli.wasm",
    )
home_cli_data = home_cli_archive.read_bytes()
home_cli_manifest["checksum"] = "sha256:" + hashlib.sha256(home_cli_data).hexdigest()
home_cli_manifest["size"] = len(home_cli_data)

browser_capsules = {
    "home": pathlib.Path(os.environ["HOME_CAPSULE_DIR"]),
    "system": pathlib.Path(os.environ["SYSTEM_CAPSULE_DIR"]),
}
for name, capsule_dir in browser_capsules.items():
    info = platform_info(name)
    release_path = info.get("release_path")
    if not release_path:
        raise SystemExit(f"{name} missing release_path for {platform}")
    archive = artifacts_dir / release_path
    with tarfile.open(archive, "w:gz") as tar:
        tar.add(capsule_dir / "capsule.json", arcname=f"{name}/capsule.json")
        tar.add(
            capsule_dir / "target/wasm32-wasip1/release" / f"{name}.wasm",
            arcname=f"{name}/{name}.wasm",
        )
        tar.add(capsule_dir / "browser", arcname=f"{name}/browser")
    data = archive.read_bytes()
    info["checksum"] = "sha256:" + hashlib.sha256(data).hexdigest()
    info["size"] = len(data)

data_capsules = {
    "documents": pathlib.Path(os.environ["DOCUMENTS_CAPSULE_DIR"]),
    "library": pathlib.Path(os.environ["LIBRARY_CAPSULE_DIR"]),
    "inbox": pathlib.Path(os.environ["INBOX_CAPSULE_DIR"]),
}
for name, capsule_dir in data_capsules.items():
    info = platform_info(name)
    release_path = info.get("release_path")
    if not release_path:
        raise SystemExit(f"{name} missing release_path for {platform}")
    archive = artifacts_dir / release_path
    with tarfile.open(archive, "w:gz") as tar:
        tar.add(capsule_dir / "capsule.json", arcname=f"{name}/capsule.json")
        tar.add(capsule_dir / "index.html", arcname=f"{name}/index.html")
    data = archive.read_bytes()
    info["checksum"] = "sha256:" + hashlib.sha256(data).hexdigest()
    info["size"] = len(data)

components_dest.parent.mkdir(parents=True, exist_ok=True)
components_dest.write_text(json.dumps(manifest, indent=2) + "\n")
PY2

HOME="$SOURCE_RUNTIME_HOME" XDG_DATA_HOME="$SOURCE_RUNTIME_XDG" "$ROOT/elastos/target/debug/elastos" serve --addr "127.0.0.1:${SOURCE_API_PORT}"     >"$SOURCE_RUNTIME_LOG" 2>&1 &
SOURCE_RUNTIME_PID=$!

for _ in $(seq 1 60); do
    [[ -f "$SOURCE_RUNTIME_COORDS" ]] && break
    sleep 0.5
done
if [[ ! -f "$SOURCE_RUNTIME_COORDS" ]]; then
    echo "[home-frontdoor] source runtime coords missing. See $SOURCE_RUNTIME_LOG" >&2
    exit 1
fi

for _ in $(seq 1 60); do
    if curl -fsS --max-time 2 "http://127.0.0.1:${SOURCE_API_PORT}/api/health" >/dev/null 2>&1; then
        break
    fi
    sleep 0.5
done
if ! curl -fsS --max-time 2 "http://127.0.0.1:${SOURCE_API_PORT}/api/health" >/dev/null 2>&1; then
    echo "[home-frontdoor] source runtime never became healthy. See $SOURCE_RUNTIME_LOG" >&2
    exit 1
fi

# **Phase 5 Day 2** — was `mapfile -t SOURCE_BOOTSTRAP < <(…)`.
# `read_pids_into_array` is named for its primary use case but
# is a generic "read newline-delimited values into an indexed
# array" helper that's bash-3.2 clean; on Linux it produces a
# byte-identical array to mapfile.
SOURCE_BOOTSTRAP=()
read_pids_into_array SOURCE_BOOTSTRAP < <(discover_source_bootstrap "$SOURCE_RUNTIME_COORDS")
SOURCE_CONNECT_TICKET="${SOURCE_BOOTSTRAP[0]:-}"
SOURCE_NODE_ID="${SOURCE_BOOTSTRAP[1]:-}"
if [[ -z "$SOURCE_CONNECT_TICKET" || -z "$SOURCE_NODE_ID" ]]; then
    echo "[home-frontdoor] failed to discover source-local trusted-source Carrier bootstrap. See $SOURCE_RUNTIME_LOG" >&2
    exit 1
fi

HOME="$HOME_DIR" XDG_DATA_HOME="$HOME_DIR/xdg-data" ELASTOS_PUBLISHER_GATEWAY="$PUBLISHER_GATEWAY" ELASTOS_MAINTAINER_DID="$MAINTAINER_DID" ELASTOS_SOURCE_CONNECT_TICKET="$SOURCE_CONNECT_TICKET" ELASTOS_PUBLISHER_NODE_ID="$SOURCE_NODE_ID" bash "$ROOT/scripts/install.sh" >/tmp/elastos-home-frontdoor-install.log

if [[ ! -f "$SOURCE_COMPONENTS_MANIFEST" ]]; then
    echo "[home-frontdoor] source components manifest missing: $SOURCE_COMPONENTS_MANIFEST" >&2
    exit 1
fi

echo "[home-frontdoor] setup home profile"
cp "$SOURCE_COMPONENTS_MANIFEST" "$HOME_DIR/xdg-data/elastos/components.json"
HOME="$HOME_DIR" XDG_DATA_HOME="$HOME_DIR/xdg-data" "$ROOT/elastos/target/debug/elastos" setup --profile home >/tmp/elastos-home-frontdoor-setup.log

INSTALLED_HOME_CLI_DIR="$HOME_DIR/xdg-data/elastos/capsules/home-cli"
if [[ ! -d "$INSTALLED_HOME_CLI_DIR" ]]; then
    echo "[home-frontdoor] installed Home CLI capsule missing after setup: $INSTALLED_HOME_CLI_DIR" >&2
    exit 1
fi
for installed in \
    "$HOME_DIR/xdg-data/elastos/capsules/home/home.wasm" \
    "$HOME_DIR/xdg-data/elastos/capsules/home/browser/index.html" \
    "$HOME_DIR/xdg-data/elastos/capsules/system/system.wasm" \
    "$HOME_DIR/xdg-data/elastos/capsules/system/browser/index.html" \
    "$HOME_DIR/xdg-data/elastos/capsules/documents/index.html" \
    "$HOME_DIR/xdg-data/elastos/capsules/library/index.html" \
    "$HOME_DIR/xdg-data/elastos/capsules/inbox/index.html"
do
    if [[ ! -f "$installed" ]]; then
        echo "[home-frontdoor] installed first-party browser capsule asset missing after setup: $installed" >&2
        exit 1
    fi
done
mv "$INSTALLED_HOME_CLI_DIR" "${INSTALLED_HOME_CLI_DIR}.installed"

echo "[home-frontdoor] prove current source elastos + current source home-cli.wasm against clean-home data"
HOME_DIR="$HOME_DIR" ROOT="$ROOT" SOURCE_COMPONENTS_MANIFEST="$SOURCE_COMPONENTS_MANIFEST" python3 - <<'PY'
import os
import pty
import select
import signal
import subprocess
import time

home = os.environ["HOME_DIR"]
root = os.environ["ROOT"]
env = os.environ.copy()
env["HOME"] = home
env["XDG_DATA_HOME"] = f"{home}/xdg-data"
# Keep the smoke hermetic: chat launched from Home must stay on the slave PTY
# instead of probing the caller's controlling terminal via /dev/tty.
env["ELASTOS_CHAT_FORCE_STDIN"] = "1"
cmd = [f"{root}/elastos/target/debug/elastos"]
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

def run_chat_case(label: str, payload: bytes) -> None:
    master, proc = launch_pty()
    try:
        read_until(label, master, lambda text: "ElastOS Home" in text, 10.0)
        send(master, b"\r\n")
        after_enter1 = read_until(
            label,
            master,
            lambda text: "Press Enter again to launch Chat" in text,
            6.0,
        )
        time.sleep(0.8)
        send(master, b"\r")
        after_enter2 = read_until(
            label,
            master,
            lambda text: "Connected to local runtime." in text and "Chat as" in text,
            20.0,
        )
        combined_chat = after_enter1 + after_enter2
        if "Chat room: #general joined." not in combined_chat or "Delivery:" not in combined_chat:
            raise SystemExit(f"{label}: second enter did not launch chat cleanly:\n{combined_chat}")
        send(master, payload, 0.5)
        after_exit = read_until(
            label,
            master,
            lambda text: "ElastOS Home" in text,
            8.0,
        )
        if "ElastOS Home" not in after_exit:
            raise SystemExit(f"{label}: exit input did not return Home:\n{after_exit}")
    finally:
        shutdown(master, proc)

def run_navigation_case() -> None:
    master, proc = launch_pty()
    try:
        initial = read_until(
            "nav",
            master,
            lambda text: "\x1b[30;46;1m Home \x1b[0m" in text and "> 1 Chat [ready]" in text,
            10.0,
        )
        if "\x1b[30;46;1m Home \x1b[0m" not in initial:
            raise SystemExit(f"nav: Home did not start on Home:\n{initial}")
        send(master, b"\x1b[C", 0.4)
        after_right = read_until(
            "nav",
            master,
            lambda text: "\x1b[30;46;1m Inbox \x1b[0m" in text,
            4.0,
        )
        if "\x1b[30;46;1m Inbox \x1b[0m" not in after_right:
            raise SystemExit(f"nav: right arrow did not switch to Inbox:\n{after_right}")
        send(master, b"\t", 0.4)
        after_tab = read_until(
            "nav",
            master,
            lambda text: "\x1b[30;46;1m People \x1b[0m" in text,
            4.0,
        )
        if "\x1b[30;46;1m People \x1b[0m" not in after_tab:
            raise SystemExit(f"nav: tab did not switch from Inbox to People:\n{after_tab}")
        send(master, b"\t", 0.4)
        after_tab_again = read_until(
            "nav",
            master,
            lambda text: "\x1b[30;46;1m Spaces \x1b[0m" in text,
            4.0,
        )
        if "\x1b[30;46;1m Spaces \x1b[0m" not in after_tab_again:
            raise SystemExit(f"nav: second tab did not switch from People to Spaces:\n{after_tab_again}")
    finally:
        shutdown(master, proc)

def run_down_navigation_case() -> None:
    master, proc = launch_pty()
    try:
        initial = read_until(
            "nav-down",
            master,
            lambda text: "> 1 Chat [ready]" in text,
            10.0,
        )
        if "> 1 Chat [ready]" not in initial:
            raise SystemExit(f"nav-down: Home did not highlight Chat first:\n{initial}")
        send(master, b"\x1b[B", 0.4)
        after_down = read_until(
            "nav-down",
            master,
            lambda text: "> 2 MyWebSite [" in text,
            4.0,
        )
        if "> 2 MyWebSite [" not in after_down:
            raise SystemExit(f"nav-down: down arrow did not move selection:\n{after_down}")
    finally:
        shutdown(master, proc)

def run_home_case(label: str, payload: bytes, expected_fragments: tuple[str, ...]) -> None:
    proc = subprocess.run(
        cmd + ["home"],
        input=payload,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        env=env,
        timeout=20,
    )
    output = proc.stdout.decode("utf-8", "replace")
    if proc.returncode != 0:
        raise SystemExit(f"{label}: home command failed:\n{output}")
    if not any(fragment in output for fragment in expected_fragments):
        joined = "\n".join(expected_fragments)
        raise SystemExit(f"{label}: expected one of:\n{joined}\n\nactual output:\n{output}")

run_navigation_case()
run_down_navigation_case()
run_chat_case("esc", b"\x1b")
run_chat_case("home", b"/home\r")
run_chat_case("quit", b"/quit\r")
run_home_case(
    "mywebsite",
    b"2\n\nq\n",
    (
        "MyWebSite is empty.",
        "MyWebSite is staged at localhost://MyWebSite.",
        "MyWebSite is not ready: missing site-provider",
    ),
)
run_home_case(
    "updates",
    b"3\nq\n",
    (
        "Returned home from Updates.",
        "Updates:",
        "Installed release is up to date.",
        "Updates could not complete the trusted-source check:",
    ),
)

print("[home-frontdoor] OK")
PY

# **Phase 5 Day 2** — Vz substrate readiness probe. Mirrors the
# Day-1 probe in `local-carrier-setup-smoke.sh`. Reports
# diagnostic findings about Vz host capability + launchable
# capsule presence in the smoke's `ELASTOS_DATA_DIR`. This is
# advisory only — failure does not fail the smoke.
#
# On Linux the probe visibly-skips because `vz_host_is_capable`
# returns false (uname -s != Darwin). On Mac the probe runs and
# reports either a capsule found (post-Phase-6) or a clean
# "no launchable capsule found yet" (current state).
if [[ "${OS_TOKEN}" == "darwin" ]]; then
    echo "[home-frontdoor] Vz substrate probe:"
    if vz_host_is_capable; then
        echo "  - Vz host capability: PASS (macOS 12+ detected via sw_vers)"
        if probe_target="$(vz_discover_launchable_capsule \
                "${HOME_DIR}/xdg-data/elastos" 2>/dev/null)" \
                && [[ -n "${probe_target}" ]]; then
            echo "  - Launchable capsule discovered: ${probe_target}"
        else
            echo "  - Launchable capsule discovered: none (expected pre-Phase 6)"
        fi
    else
        echo "  - Vz host capability: SKIP (not macOS 12+ or sw_vers unavailable)"
    fi
else
    echo "[home-frontdoor] Vz substrate probe: SKIP (host is ${OS_TOKEN}, not Darwin)"
fi
