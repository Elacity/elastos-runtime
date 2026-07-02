#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${REPO_ROOT}/scripts/lib/public-install-guards.sh"
PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"
FORCE_RELAY_ONLY="${ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY:-0}"
HOME_DIR="$(mktemp -d /tmp/elastos-public-identity-XXXXXX)"
trap 'rm -rf "$HOME_DIR"' EXIT

echo "[public-identity] install from public gateway"
HOME="${HOME_DIR}" \
XDG_DATA_HOME="${HOME_DIR}/xdg-data" \
ELASTOS_PUBLISHER_GATEWAY="${PUBLISHER_GATEWAY}" \
bash -lc 'mkdir -p "$HOME" "$XDG_DATA_HOME" && curl -fsSL "${ELASTOS_PUBLISHER_GATEWAY%/}/install.sh" | bash' \
    >/tmp/elastos-public-identity-install.log

INSTALLED_BIN="${HOME_DIR}/.local/bin/elastos"
RUN_BIN="${ELASTOS_BIN_OVERRIDE:-${INSTALLED_BIN}}"
DATA_DIR="${HOME_DIR}/xdg-data/elastos"
INSTALLED_COMPONENTS_MANIFEST="${DATA_DIR}/components.json"
SOURCES_PATH="${DATA_DIR}/sources.json"

if [[ ! -x "${INSTALLED_BIN}" ]]; then
    echo "[public-identity] installed binary missing: ${INSTALLED_BIN}" >&2
    exit 1
fi
if [[ ! -x "${RUN_BIN}" ]]; then
    echo "[public-identity] run binary missing: ${RUN_BIN}" >&2
    exit 1
fi

echo "[public-identity] prove stamped trusted source"
SOURCE_OUTPUT="$(
    HOME="${HOME_DIR}" \
    XDG_DATA_HOME="${HOME_DIR}/xdg-data" \
    "${INSTALLED_BIN}" source show
)"
echo "${SOURCE_OUTPUT}"
if ! grep -q "Bootstrap: peer ticket configured" <<<"${SOURCE_OUTPUT}"; then
    echo "[public-identity] expected stamped Carrier bootstrap ticket missing from source show" >&2
    exit 1
fi
if grep -q "Node ID:   none" <<<"${SOURCE_OUTPUT}"; then
    echo "[public-identity] expected stamped publisher node id missing from source show" >&2
    exit 1
fi

if [[ "${FORCE_RELAY_ONLY}" == "1" ]]; then
    echo "[public-identity] remove gateway override and direct addrs to force relay-only Carrier setup"
    SOURCES_PATH="${SOURCES_PATH}" python3 - <<'PY'
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
    echo "[public-identity] use stamped trusted-source transports"
fi

echo "[public-identity] run default setup"
guard_branch_binary_requires_checksummed_public_manifest "${INSTALLED_COMPONENTS_MANIFEST}" "[public-identity]"
HOME="${HOME_DIR}" \
XDG_DATA_HOME="${HOME_DIR}/xdg-data" \
ELASTOS_COMPONENTS_MANIFEST="${INSTALLED_COMPONENTS_MANIFEST}" \
"${RUN_BIN}" setup >/tmp/elastos-public-identity-setup.log

echo "[public-identity] prove DID-backed identity contract"
ELASTOS_COMPONENTS_MANIFEST="${INSTALLED_COMPONENTS_MANIFEST}" \
ELASTOS_BIN="${RUN_BIN}" \
ELASTOS_IDENTITY_HOME="${HOME_DIR}" \
ELASTOS_IDENTITY_XDG_DATA_HOME="${HOME_DIR}/xdg-data" \
ELASTOS_DATA_DIR="${DATA_DIR}" \
bash "${REPO_ROOT}/scripts/identity-profile-smoke.sh"

echo "[public-identity] OK"
