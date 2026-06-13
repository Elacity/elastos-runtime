#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ELASTOS_ROOT="${REPO_ROOT}/elastos"
ELASTOS_BIN="${ELASTOS_ROOT}/target/debug/elastos"

if [[ "$(uname -s)" != "Linux" ]]; then
    echo "local-carrier-setup-smoke currently supports Linux only." >&2
    exit 1
fi

case "$(uname -m)" in
    x86_64) SETUP_PLATFORM="linux-amd64" ;;
    aarch64|arm64) SETUP_PLATFORM="linux-arm64" ;;
    *)
        echo "Unsupported machine architecture: $(uname -m)" >&2
        exit 1
        ;;
esac

TEST_ROOT="${ELASTOS_LOCAL_TEST_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/elastos-local-carrier-setup.XXXXXX")}"
XDG_DATA_HOME="${TEST_ROOT}/xdg-data"
DATA_DIR="${XDG_DATA_HOME}/elastos"
PUBLISHER_ROOT="${DATA_DIR}/ElastOS/SystemServices/Publisher"
ARTIFACTS_DIR="${PUBLISHER_ROOT}/artifacts"
LOG_PATH="${TEST_ROOT}/serve.log"
API_PORT="$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"

stop_source_runtime() {
    if [[ -z "${SERVE_PID:-}" ]]; then
        rm -f "${RUNTIME_COORDS:-}"
        return 0
    fi

    if ! kill -0 "${SERVE_PID}" 2>/dev/null; then
        wait "${SERVE_PID}" 2>/dev/null || true
        rm -f "${RUNTIME_COORDS:-}"
        unset SERVE_PID
        return 0
    fi

    kill -- "-${SERVE_PID}" 2>/dev/null || kill "${SERVE_PID}" 2>/dev/null || true
    for _ in $(seq 1 20); do
        if ! kill -0 "${SERVE_PID}" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done
    if kill -0 "${SERVE_PID}" 2>/dev/null; then
        kill -KILL -- "-${SERVE_PID}" 2>/dev/null || kill -KILL "${SERVE_PID}" 2>/dev/null || true
    fi
    rm -f "${RUNTIME_COORDS:-}"
    unset SERVE_PID
}

kill_temp_processes() {
    local root="$1"
    local skip_pid="${2:-}"
    mapfile -t pids < <(pgrep -f "$root" || true)
    for pid in "${pids[@]}"; do
        [[ "$pid" == "$$" ]] && continue
        [[ -n "${skip_pid}" && "$pid" == "${skip_pid}" ]] && continue
        kill "$pid" 2>/dev/null || true
    done
    sleep 0.2
    for pid in "${pids[@]}"; do
        [[ "$pid" == "$$" ]] && continue
        [[ -n "${skip_pid}" && "$pid" == "${skip_pid}" ]] && continue
        if kill -0 "$pid" 2>/dev/null; then
            kill -9 "$pid" 2>/dev/null || true
        fi
    done
    return 0
}

cleanup() {
    stop_source_runtime
    kill_temp_processes "${TEST_ROOT}"
    return 0
}
trap cleanup EXIT

echo "[local-carrier-setup] test root: ${TEST_ROOT}"
echo "[local-carrier-setup] building current binary and first-party Home core assets"

(cd "${ELASTOS_ROOT}" && cargo build -p elastos-server)
(cd "${REPO_ROOT}/elastos/capsules/shell" && cargo build --release)
(cd "${REPO_ROOT}/elastos/capsules/localhost-provider" && cargo build --release)
(cd "${REPO_ROOT}/capsules/did-provider" && cargo build --release)
(cd "${REPO_ROOT}/capsules/webspace-provider" && cargo build --release)
(cd "${REPO_ROOT}/capsules/object-provider" && cargo build --release)
(cd "${REPO_ROOT}/capsules/home-cli" && cargo build --target wasm32-wasip1 --release)
for capsule in \
    home \
    system \
    browser \
    documents \
    gba-emulator \
    inbox \
    library \
    marketplace \
    wallet \
    wallet-metamask \
    wallet-unisat \
    wallet-walletconnect
do
    (cd "${REPO_ROOT}/capsules/${capsule}" && cargo build --target wasm32-wasip1 --release)
done

mkdir -p "${ARTIFACTS_DIR}"
mkdir -p "${DATA_DIR}/bin"

# `elastos serve` now fails closed unless localhost-provider is already
# installed. Seed the one required host provider before starting the local
# source runtime; the rest of the setup still proves Carrier-backed install.
install -m 755 \
    "${REPO_ROOT}/elastos/target/release/localhost-provider" \
    "${DATA_DIR}/bin/localhost-provider"

COMPONENTS_SRC="${REPO_ROOT}/components.json" \
COMPONENTS_DEST="${DATA_DIR}/components.json" \
DATA_DIR="${DATA_DIR}" \
PUBLISHER_ROOT="${PUBLISHER_ROOT}" \
SETUP_PLATFORM="${SETUP_PLATFORM}" \
SHELL_BIN="${REPO_ROOT}/elastos/target/release/shell" \
LOCALHOST_PROVIDER_BIN="${REPO_ROOT}/elastos/target/release/localhost-provider" \
DID_PROVIDER_BIN="${REPO_ROOT}/capsules/did-provider/target/release/did-provider" \
WEBSPACE_PROVIDER_BIN="${REPO_ROOT}/capsules/webspace-provider/target/release/webspace-provider" \
OBJECT_PROVIDER_BIN="${REPO_ROOT}/capsules/object-provider/target/release/object-provider" \
HOME_CLI_DIR="${REPO_ROOT}/capsules/home-cli" \
HOME_CAPSULE_DIR="${REPO_ROOT}/capsules/home" \
SYSTEM_CAPSULE_DIR="${REPO_ROOT}/capsules/system" \
BROWSER_CAPSULE_DIR="${REPO_ROOT}/capsules/browser" \
DOCUMENTS_CAPSULE_DIR="${REPO_ROOT}/capsules/documents" \
GBA_EMULATOR_CAPSULE_DIR="${REPO_ROOT}/capsules/gba-emulator" \
LIBRARY_CAPSULE_DIR="${REPO_ROOT}/capsules/library" \
MARKETPLACE_CAPSULE_DIR="${REPO_ROOT}/capsules/marketplace" \
INBOX_CAPSULE_DIR="${REPO_ROOT}/capsules/inbox" \
WALLET_CAPSULE_DIR="${REPO_ROOT}/capsules/wallet" \
WALLET_METAMASK_CAPSULE_DIR="${REPO_ROOT}/capsules/wallet-metamask" \
WALLET_UNISAT_CAPSULE_DIR="${REPO_ROOT}/capsules/wallet-unisat" \
WALLET_WALLETCONNECT_CAPSULE_DIR="${REPO_ROOT}/capsules/wallet-walletconnect" \
python3 - <<'PY'
import hashlib
import json
import os
import pathlib
import shutil
import tarfile

components_src = pathlib.Path(os.environ["COMPONENTS_SRC"])
components_dest = pathlib.Path(os.environ["COMPONENTS_DEST"])
data_dir = pathlib.Path(os.environ["DATA_DIR"])
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
    "object-provider": pathlib.Path(os.environ["OBJECT_PROVIDER_BIN"]),
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
    "browser": pathlib.Path(os.environ["BROWSER_CAPSULE_DIR"]),
    "documents": pathlib.Path(os.environ["DOCUMENTS_CAPSULE_DIR"]),
    "gba-emulator": pathlib.Path(os.environ["GBA_EMULATOR_CAPSULE_DIR"]),
    "inbox": pathlib.Path(os.environ["INBOX_CAPSULE_DIR"]),
    "library": pathlib.Path(os.environ["LIBRARY_CAPSULE_DIR"]),
    "marketplace": pathlib.Path(os.environ["MARKETPLACE_CAPSULE_DIR"]),
    "wallet": pathlib.Path(os.environ["WALLET_CAPSULE_DIR"]),
    "wallet-metamask": pathlib.Path(os.environ["WALLET_METAMASK_CAPSULE_DIR"]),
    "wallet-unisat": pathlib.Path(os.environ["WALLET_UNISAT_CAPSULE_DIR"]),
    "wallet-walletconnect": pathlib.Path(os.environ["WALLET_WALLETCONNECT_CAPSULE_DIR"]),
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

components_dest.parent.mkdir(parents=True, exist_ok=True)
components_dest.write_text(json.dumps(manifest, indent=2) + "\n")
PY

echo "[local-carrier-setup] staged local artifacts into ${ARTIFACTS_DIR}"

mkdir -p "${DATA_DIR}"
SERVE_PID="$(
    ELASTOS_ROOT="${ELASTOS_ROOT}" \
    ELASTOS_BIN="${ELASTOS_BIN}" \
    XDG_DATA_HOME="${XDG_DATA_HOME}" \
    API_PORT="${API_PORT}" \
    LOG_PATH="${LOG_PATH}" \
    python3 - <<'PY'
import os
import subprocess

env = os.environ.copy()
with open(os.environ["LOG_PATH"], "ab") as log:
    proc = subprocess.Popen(
        [
            os.environ["ELASTOS_BIN"],
            "serve",
            "--addr",
            f'127.0.0.1:{os.environ["API_PORT"]}',
        ],
        cwd=os.environ["ELASTOS_ROOT"],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=log,
        stderr=log,
        start_new_session=True,
        close_fds=True,
    )
print(proc.pid)
PY
)"

RUNTIME_COORDS="${DATA_DIR}/runtime-coords.json"
for _ in $(seq 1 60); do
    if [[ -f "${RUNTIME_COORDS}" ]]; then
        break
    fi
    sleep 0.5
done

if [[ ! -f "${RUNTIME_COORDS}" ]]; then
    echo "runtime-coords.json was not created. See ${LOG_PATH}" >&2
    exit 1
fi

SOURCE_BOOTSTRAP_FILE="${TEST_ROOT}/source-bootstrap.txt"
for _ in $(seq 1 60); do
    if RUNTIME_COORDS="${RUNTIME_COORDS}" SOURCE_BOOTSTRAP_FILE="${SOURCE_BOOTSTRAP_FILE}" python3 - <<'PY'
import json
import os
import urllib.request
import urllib.error

coords = json.loads(open(os.environ["RUNTIME_COORDS"]).read())
api_url = coords["api_url"]
secret = coords["attach_secret"]

try:
    with urllib.request.urlopen(api_url + "/api/health", timeout=2) as resp:
        if resp.status != 200:
            raise RuntimeError("runtime not healthy yet")

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

    with open(os.environ["SOURCE_BOOTSTRAP_FILE"], "w", encoding="utf-8") as f:
        f.write(body["data"]["ticket"] + "\n")
        f.write(body["data"]["node_id"] + "\n")
except Exception:
    raise SystemExit(1)
PY
    then
        break
    fi
    sleep 0.5
done

if [[ ! -f "${SOURCE_BOOTSTRAP_FILE}" ]]; then
    echo "failed to obtain local Carrier bootstrap details. See ${LOG_PATH}" >&2
    exit 1
fi

readarray -t SOURCE_BOOTSTRAP < "${SOURCE_BOOTSTRAP_FILE}"
CONNECT_TICKET="${SOURCE_BOOTSTRAP[0]:-}"
NODE_ID="${SOURCE_BOOTSTRAP[1]:-}"

if [[ -z "${CONNECT_TICKET}" || -z "${NODE_ID}" ]]; then
    echo "failed to obtain local Carrier bootstrap details. See ${LOG_PATH}" >&2
    exit 1
fi

SOURCES_PATH="${DATA_DIR}/sources.json"
CONNECT_TICKET="${CONNECT_TICKET}" \
NODE_ID="${NODE_ID}" \
ELASTOS_BIN_PATH="${ELASTOS_BIN}" \
SOURCES_PATH="${SOURCES_PATH}" \
python3 - <<'PY'
import json
import os

sources = {
    "schema": "elastos.trusted-sources/v1",
    "default_source": "default",
    "sources": [
        {
            "name": "default",
            "publisher_dids": ["did:key:local-carrier-test"],
            "channel": "stable",
            "discovery_uri": "elastos://source/stable/local-carrier-test",
            "connect_ticket": os.environ["CONNECT_TICKET"],
            "publisher_node_id": os.environ["NODE_ID"],
            "ipns_name": "",
            "gateways": [],
            "install_path": os.environ["ELASTOS_BIN_PATH"],
            "installed_version": "",
            "head_cid": "",
        }
    ],
}

with open(os.environ["SOURCES_PATH"], "w", encoding="utf-8") as f:
    json.dump(sources, f, indent=2)
    f.write("\n")
PY

echo "[local-carrier-setup] running Carrier-only setup smoke"
(
    cd "${ELASTOS_ROOT}"
    XDG_DATA_HOME="${XDG_DATA_HOME}" \
    "${ELASTOS_BIN}" setup
)

stop_source_runtime

for installed in \
    "${DATA_DIR}/bin/shell" \
    "${DATA_DIR}/bin/localhost-provider" \
    "${DATA_DIR}/bin/did-provider" \
    "${DATA_DIR}/bin/webspace-provider" \
    "${DATA_DIR}/bin/object-provider" \
    "${DATA_DIR}/capsules/home-cli/home-cli.wasm" \
    "${DATA_DIR}/capsules/home-cli/capsule.json" \
    "${DATA_DIR}/capsules/home/home.wasm" \
    "${DATA_DIR}/capsules/home/browser/index.html" \
    "${DATA_DIR}/capsules/system/system.wasm" \
    "${DATA_DIR}/capsules/system/browser/index.html" \
    "${DATA_DIR}/capsules/browser/browser.wasm" \
    "${DATA_DIR}/capsules/browser/browser/index.html" \
    "${DATA_DIR}/capsules/documents/documents.wasm" \
    "${DATA_DIR}/capsules/documents/browser/index.html" \
    "${DATA_DIR}/capsules/gba-emulator/gba-emulator.wasm" \
    "${DATA_DIR}/capsules/gba-emulator/browser/index.html" \
    "${DATA_DIR}/capsules/inbox/inbox.wasm" \
    "${DATA_DIR}/capsules/inbox/browser/index.html" \
    "${DATA_DIR}/capsules/library/library.wasm" \
    "${DATA_DIR}/capsules/library/browser/index.html" \
    "${DATA_DIR}/capsules/library/browser/library.css" \
    "${DATA_DIR}/capsules/library/browser/src/app.js" \
    "${DATA_DIR}/capsules/library/browser/icons/folder.svg" \
    "${DATA_DIR}/capsules/marketplace/marketplace.wasm" \
    "${DATA_DIR}/capsules/marketplace/browser/index.html" \
    "${DATA_DIR}/capsules/marketplace/browser/marketplace.css" \
    "${DATA_DIR}/capsules/marketplace/browser/marketplace.js" \
    "${DATA_DIR}/capsules/wallet/wallet.wasm" \
    "${DATA_DIR}/capsules/wallet/browser/index.html" \
    "${DATA_DIR}/capsules/wallet-metamask/wallet-metamask.wasm" \
    "${DATA_DIR}/capsules/wallet-metamask/browser/index.html" \
    "${DATA_DIR}/capsules/wallet-unisat/wallet-unisat.wasm" \
    "${DATA_DIR}/capsules/wallet-unisat/browser/index.html" \
    "${DATA_DIR}/capsules/wallet-walletconnect/wallet-walletconnect.wasm" \
    "${DATA_DIR}/capsules/wallet-walletconnect/browser/index.html"
do
    if [[ ! -f "${installed}" ]]; then
        echo "expected installed file missing: ${installed}" >&2
        exit 1
    fi
done

STATUS_OUT="${TEST_ROOT}/home-status.txt"
(
    cd "${ELASTOS_ROOT}"
    XDG_DATA_HOME="${XDG_DATA_HOME}" \
    "${ELASTOS_BIN}" home --status >"${STATUS_OUT}"
)
grep -q "ElastOS Home" "${STATUS_OUT}" || {
    echo "expected home status output missing from ${STATUS_OUT}" >&2
    exit 1
}

HOME_OUT="${TEST_ROOT}/home.txt"
(
    cd "${ELASTOS_ROOT}"
    printf 'q\n' | XDG_DATA_HOME="${XDG_DATA_HOME}" \
    "${ELASTOS_BIN}" >"${HOME_OUT}"
)
grep -q "ElastOS Home" "${HOME_OUT}" || {
    echo "expected home output missing from ${HOME_OUT}" >&2
    exit 1
}

echo "[local-carrier-setup] OK"
echo "[local-carrier-setup] temp data dir: ${DATA_DIR}"
echo "[local-carrier-setup] runtime log:   ${LOG_PATH}"
echo "[local-carrier-setup] inspect with:"
echo "  XDG_DATA_HOME=\"${XDG_DATA_HOME}\" \"${ELASTOS_BIN}\""
