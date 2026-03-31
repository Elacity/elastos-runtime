#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVER_MANIFEST="${ROOT}/elastos/Cargo.toml"
DEFAULT_ELASTOS_BIN="${ROOT}/elastos/target/debug/elastos"
ELASTOS_BIN="${ELASTOS_BIN:-${DEFAULT_ELASTOS_BIN}}"
HOME_DIR="${ELASTOS_LOCAL_IDENTITY_HOME:-$(mktemp -d /tmp/elastos-local-identity-XXXXXX)}"
PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"
MAINTAINER_DID="${ELASTOS_MAINTAINER_DID:-did:key:z6Mkf2nCJ1pcN4JioAxHEiyDsPC298QFtn2Dgg9tjt2ezHeK}"
SOURCE_COMPONENTS_MANIFEST="${ROOT}/components.json"
OPERATOR_HOME="${HOME}"
SKIP_BUILD=0

usage() {
    cat <<'EOF'
Usage:
  bash scripts/local-identity-profile-smoke.sh
  bash scripts/local-identity-profile-smoke.sh --skip-build

What it proves:
  1. clean-home install from the stamped installer with live Carrier bootstrap
  2. source-local `setup --profile pc2`
  3. DID nickname set/get/show
  4. PC2 status snapshot reflects the same nickname
  5. `chat` defaults to the DID nickname
  6. `codex` resolves to a persona DID distinct from the root DID
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

discover_source_bootstrap() {
    local coords_file="${OPERATOR_HOME}/.local/share/elastos/runtime-coords.json"
    if [[ ! -f "${coords_file}" ]]; then
        echo "[local-identity-profile] runtime coords missing: ${coords_file}" >&2
        return 1
    fi

    RUNTIME_COORDS="${coords_file}" python3 - <<'PY'
import json
import os
import urllib.request

coords = json.loads(open(os.environ["RUNTIME_COORDS"], "r", encoding="utf-8").read())
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

if [[ "${SKIP_BUILD}" -eq 0 ]]; then
    echo "[local-identity-profile] build elastos binary"
    cargo build -q --manifest-path "${SERVER_MANIFEST}" -p elastos-server
fi

if [[ ! -x "${ELASTOS_BIN}" ]]; then
    echo "[local-identity-profile] missing source-local elastos binary: ${ELASTOS_BIN}" >&2
    exit 1
fi

echo "[local-identity-profile] discover live trusted-source bootstrap"
mapfile -t SOURCE_BOOTSTRAP < <(discover_source_bootstrap)
SOURCE_CONNECT_TICKET="${SOURCE_BOOTSTRAP[0]:-}"
SOURCE_NODE_ID="${SOURCE_BOOTSTRAP[1]:-}"
if [[ -z "${SOURCE_CONNECT_TICKET}" || -z "${SOURCE_NODE_ID}" ]]; then
    echo "[local-identity-profile] failed to discover live trusted-source Carrier bootstrap" >&2
    exit 1
fi

echo "[local-identity-profile] install clean temp-home runtime"
HOME="${HOME_DIR}" \
XDG_DATA_HOME="${HOME_DIR}/xdg-data" \
ELASTOS_DATA_DIR="${HOME_DIR}/xdg-data/elastos" \
ELASTOS_PUBLISHER_GATEWAY="${PUBLISHER_GATEWAY}" \
ELASTOS_MAINTAINER_DID="${MAINTAINER_DID}" \
ELASTOS_SOURCE_CONNECT_TICKET="${SOURCE_CONNECT_TICKET}" \
ELASTOS_PUBLISHER_NODE_ID="${SOURCE_NODE_ID}" \
bash "${ROOT}/scripts/install.sh" >/tmp/elastos-local-identity-install.log

echo "[local-identity-profile] setup pc2 profile from source"
mkdir -p "${HOME_DIR}/xdg-data/elastos"
cp "${SOURCE_COMPONENTS_MANIFEST}" "${HOME_DIR}/xdg-data/elastos/components.json"
HOME="${HOME_DIR}" \
XDG_DATA_HOME="${HOME_DIR}/xdg-data" \
ELASTOS_DATA_DIR="${HOME_DIR}/xdg-data/elastos" \
"${ELASTOS_BIN}" setup --profile pc2 >/tmp/elastos-local-identity-setup.log

echo "[local-identity-profile] prove DID-backed identity contract"
ELASTOS_BIN="${ELASTOS_BIN}" \
ELASTOS_IDENTITY_HOME="${HOME_DIR}" \
ELASTOS_IDENTITY_XDG_DATA_HOME="${HOME_DIR}/xdg-data" \
ELASTOS_DATA_DIR="${HOME_DIR}/xdg-data/elastos" \
bash "${ROOT}/scripts/identity-profile-smoke.sh"
