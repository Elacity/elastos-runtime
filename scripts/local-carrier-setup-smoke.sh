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

cleanup() {
    if [[ -n "${SERVE_PID:-}" ]] && kill -0 "${SERVE_PID}" 2>/dev/null; then
        kill "${SERVE_PID}" 2>/dev/null || true
        wait "${SERVE_PID}" 2>/dev/null || true
    fi
}
trap cleanup EXIT

echo "[local-carrier-setup] test root: ${TEST_ROOT}"
echo "[local-carrier-setup] building current binary and first-party PC2 core assets"

(cd "${ELASTOS_ROOT}" && cargo build -p elastos-server)
(cd "${REPO_ROOT}/elastos/capsules/shell" && cargo build --release)
(cd "${REPO_ROOT}/elastos/capsules/localhost-provider" && cargo build --release)
(cd "${REPO_ROOT}/capsules/did-provider" && cargo build --release)
(cd "${REPO_ROOT}/capsules/webspace-provider" && cargo build --release)
(cd "${REPO_ROOT}/capsules/pc2" && cargo build --target wasm32-wasip1 --release)

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
PC2_DIR="${REPO_ROOT}/capsules/pc2" \
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
mapping = {
    "shell": pathlib.Path(os.environ["SHELL_BIN"]),
    "localhost-provider": pathlib.Path(os.environ["LOCALHOST_PROVIDER_BIN"]),
    "did-provider": pathlib.Path(os.environ["DID_PROVIDER_BIN"]),
    "webspace-provider": pathlib.Path(os.environ["WEBSPACE_PROVIDER_BIN"]),
}

for name, src in mapping.items():
    if not src.is_file():
        raise SystemExit(f"missing built artifact for {name}: {src}")
    info = manifest["external"][name]["platforms"][platform]
    release_path = info.get("release_path")
    if not release_path:
        raise SystemExit(f"{name} missing release_path for {platform}")
    dest = artifacts_dir / release_path
    shutil.copy2(src, dest)
    data = dest.read_bytes()
    info["checksum"] = "sha256:" + hashlib.sha256(data).hexdigest()
    info["size"] = len(data)

pc2_dir = pathlib.Path(os.environ["PC2_DIR"])
pc2_manifest = manifest["external"]["pc2"]["platforms"][platform]
pc2_release_path = pc2_manifest.get("release_path")
if not pc2_release_path:
    raise SystemExit(f"pc2 missing release_path for {platform}")
pc2_archive = artifacts_dir / pc2_release_path
with tarfile.open(pc2_archive, "w:gz") as tar:
    tar.add(pc2_dir / "capsule.json", arcname="pc2/capsule.json")
    tar.add(
        pc2_dir / "target/wasm32-wasip1/release/pc2.wasm",
        arcname="pc2/pc2.wasm",
    )
pc2_data = pc2_archive.read_bytes()
pc2_manifest["checksum"] = "sha256:" + hashlib.sha256(pc2_data).hexdigest()
pc2_manifest["size"] = len(pc2_data)

components_dest.parent.mkdir(parents=True, exist_ok=True)
components_dest.write_text(json.dumps(manifest, indent=2) + "\n")
PY

echo "[local-carrier-setup] staged local artifacts into ${ARTIFACTS_DIR}"

mkdir -p "${DATA_DIR}"
(
    cd "${ELASTOS_ROOT}"
    XDG_DATA_HOME="${XDG_DATA_HOME}" \
    "${ELASTOS_BIN}" serve --addr "127.0.0.1:${API_PORT}" >"${LOG_PATH}" 2>&1
) &
SERVE_PID=$!

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

for installed in \
    "${DATA_DIR}/bin/shell" \
    "${DATA_DIR}/bin/localhost-provider" \
    "${DATA_DIR}/bin/did-provider" \
    "${DATA_DIR}/bin/webspace-provider" \
    "${DATA_DIR}/capsules/pc2/pc2.wasm" \
    "${DATA_DIR}/capsules/pc2/capsule.json"
do
    if [[ ! -f "${installed}" ]]; then
        echo "expected installed file missing: ${installed}" >&2
        exit 1
    fi
done

STATUS_OUT="${TEST_ROOT}/pc2-status.txt"
(
    cd "${ELASTOS_ROOT}"
    XDG_DATA_HOME="${XDG_DATA_HOME}" \
    "${ELASTOS_BIN}" pc2 --status >"${STATUS_OUT}"
)
grep -q "ElastOS PC2" "${STATUS_OUT}" || {
    echo "expected pc2 status output missing from ${STATUS_OUT}" >&2
    exit 1
}

HOME_OUT="${TEST_ROOT}/pc2-home.txt"
(
    cd "${ELASTOS_ROOT}"
    printf 'q\n' | XDG_DATA_HOME="${XDG_DATA_HOME}" \
    "${ELASTOS_BIN}" >"${HOME_OUT}"
)
grep -q "ElastOS PC2" "${HOME_OUT}" || {
    echo "expected pc2 home output missing from ${HOME_OUT}" >&2
    exit 1
}

echo "[local-carrier-setup] OK"
echo "[local-carrier-setup] temp data dir: ${DATA_DIR}"
echo "[local-carrier-setup] runtime log:   ${LOG_PATH}"
echo "[local-carrier-setup] inspect with:"
echo "  XDG_DATA_HOME=\"${XDG_DATA_HOME}\" \"${ELASTOS_BIN}\""
