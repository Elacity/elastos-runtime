#!/usr/bin/env bash
set -euo pipefail

PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"
TEST_ROOT="${ELASTOS_PC2_CHAT_REUSE_TEST_ROOT:-$(mktemp -d /tmp/elastos-pc2-chat-reuse.XXXXXX)}"
PC2_LOG="${TEST_ROOT}/pc2.txt"
CHAT_LOG="${TEST_ROOT}/chat.txt"
INSTALL_LOG="${TEST_ROOT}/install.txt"
SETUP_LOG="${TEST_ROOT}/setup.txt"
HOME_DIR="${TEST_ROOT}/home"
XDG_DIR="${HOME_DIR}/xdg-data"
DATA_DIR="${XDG_DIR}/elastos"
BIN="${HOME_DIR}/.local/bin/elastos"
RUN_BIN=""
PC2_COORDS="${DATA_DIR}/pc2-runtime-coords.json"
CHAT_COORDS="${DATA_DIR}/runtime-coords.json"
PC2_PID=""

cleanup() {
    if [[ -n "${PC2_PID}" ]] && kill -0 "${PC2_PID}" 2>/dev/null; then
        kill "${PC2_PID}" 2>/dev/null || true
        wait "${PC2_PID}" 2>/dev/null || true
    fi
    if [[ -z "${ELASTOS_PC2_CHAT_REUSE_TEST_ROOT:-}" ]]; then
        rm -rf "${TEST_ROOT}"
    fi
}
trap cleanup EXIT

wait_for_file() {
    local path="$1"
    for _ in $(seq 1 80); do
        [[ -f "${path}" ]] && return 0
        sleep 0.25
    done
    return 1
}

wait_for_runtime_health() {
    local coords_path="$1"
    for _ in $(seq 1 80); do
        if python3 - "$coords_path" <<'PY'
import json
import sys
import urllib.request

path = sys.argv[1]
with open(path, "r", encoding="utf-8") as fh:
    coords = json.load(fh)

try:
    with urllib.request.urlopen(coords["api_url"] + "/api/health", timeout=1) as resp:
        if resp.status == 200:
            raise SystemExit(0)
except Exception:
    pass
raise SystemExit(1)
PY
        then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

assert_log_contains() {
    local path="$1"
    local pattern="$2"
    if ! grep -Fq -- "$pattern" "$path"; then
        echo "[pc2-chat-reuse] expected '$pattern' in $path" >&2
        sed -n '1,240p' "$path" >&2 || true
        exit 1
    fi
}

assert_log_not_contains() {
    local path="$1"
    local pattern="$2"
    if grep -Fq -- "$pattern" "$path"; then
        echo "[pc2-chat-reuse] unexpected '$pattern' in $path" >&2
        sed -n '1,240p' "$path" >&2 || true
        exit 1
    fi
}

mkdir -p "${HOME_DIR}"

echo "[pc2-chat-reuse] install"
HOME="${HOME_DIR}" \
XDG_DATA_HOME="${XDG_DIR}" \
ELASTOS_PUBLISHER_GATEWAY="${PUBLISHER_GATEWAY}" \
bash -lc 'curl -fsSL "${ELASTOS_PUBLISHER_GATEWAY%/}/install.sh" | bash' \
    >"${INSTALL_LOG}" 2>&1

if [[ ! -x "${BIN}" ]]; then
    echo "[pc2-chat-reuse] installed binary missing: ${BIN}" >&2
    exit 1
fi

RUN_BIN="${ELASTOS_BIN_OVERRIDE:-${BIN}}"
if [[ ! -x "${RUN_BIN}" ]]; then
    echo "[pc2-chat-reuse] run binary missing: ${RUN_BIN}" >&2
    exit 1
fi

echo "[pc2-chat-reuse] setup"
HOME="${HOME_DIR}" \
XDG_DATA_HOME="${XDG_DIR}" \
"${RUN_BIN}" setup --profile pc2 \
    >"${SETUP_LOG}" 2>&1

echo "[pc2-chat-reuse] start elastos (pc2) in background"
HOME="${HOME_DIR}" \
XDG_DATA_HOME="${XDG_DIR}" \
timeout 20s script -q "${PC2_LOG}" -c "${RUN_BIN}" >/dev/null 2>&1 &
PC2_PID=$!

wait_for_file "${PC2_COORDS}" || {
    echo "[pc2-chat-reuse] pc2 runtime coords missing: ${PC2_COORDS}" >&2
    sed -n '1,240p' "${PC2_LOG}" >&2 || true
    exit 1
}

wait_for_runtime_health "${PC2_COORDS}" || {
    echo "[pc2-chat-reuse] pc2 runtime did not become healthy: ${PC2_COORDS}" >&2
    sed -n '1,240p' "${PC2_LOG}" >&2 || true
    exit 1
}

echo "[pc2-chat-reuse] run elastos chat and expect pc2 runtime reuse"
(
    sleep 5
    echo "/quit"
) | HOME="${HOME_DIR}" \
    XDG_DATA_HOME="${XDG_DIR}" \
    ELASTOS_CHAT_FORCE_STDIN=1 \
    "${RUN_BIN}" chat --nick reuse-smoke >"${CHAT_LOG}" 2>&1

assert_log_contains "${CHAT_LOG}" "Reusing local pc2 runtime for chat."
assert_log_not_contains "${CHAT_LOG}" "No runtime found. Starting local chat runtime..."

if [[ -f "${CHAT_COORDS}" ]]; then
    echo "[pc2-chat-reuse] standalone chat runtime coords should not have been created: ${CHAT_COORDS}" >&2
    sed -n '1,240p' "${CHAT_LOG}" >&2 || true
    exit 1
fi

echo "[pc2-chat-reuse] OK"
echo "  pc2 log:  ${PC2_LOG}"
echo "  chat log: ${CHAT_LOG}"
echo "--- chat log ---"
sed -n '1,220p' "${CHAT_LOG}"
