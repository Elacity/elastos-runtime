#!/usr/bin/env bash
set -euo pipefail

PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"
TEST_ROOT="${ELASTOS_NATIVE_CHAT_TEST_ROOT:-$(mktemp -d /tmp/elastos-installed-native-chat.XXXXXX)}"
SMOKE_ID="$(python3 - <<'PY'
import uuid
print(uuid.uuid4().hex[:8])
PY
)"
ALICE_NICK="alice-${SMOKE_ID}"
BOB_NICK="bob-${SMOKE_ID}"
ALICE_MSG="hello-from-${ALICE_NICK}"
BOB_MSG="hello-from-${BOB_NICK}"

declare -A HOME_DIR XDG_DIR BIN_PATH LOG_PATH PID

cleanup() {
    for name in "${!PID[@]}"; do
        if [[ -n "${PID[$name]:-}" ]] && kill -0 "${PID[$name]}" 2>/dev/null; then
            kill "${PID[$name]}" 2>/dev/null || true
            wait "${PID[$name]}" 2>/dev/null || true
        fi
    done
    if [[ -z "${ELASTOS_NATIVE_CHAT_TEST_ROOT:-}" ]]; then
        rm -rf "${TEST_ROOT}"
    fi
}
trap cleanup EXIT

assert_log_contains() {
    local path="$1"
    local pattern="$2"
    if ! grep -Fq -- "$pattern" "$path"; then
        echo "[installed-native-chat] expected '$pattern' in $path" >&2
        sed -n '1,240p' "$path" >&2 || true
        exit 1
    fi
}

assert_log_not_contains() {
    local path="$1"
    local pattern="$2"
    if grep -Fq -- "$pattern" "$path"; then
        echo "[installed-native-chat] unexpected '$pattern' in $path" >&2
        sed -n '1,240p' "$path" >&2 || true
        exit 1
    fi
}

prepare_home() {
    local name="$1"
    HOME_DIR["$name"]="${TEST_ROOT}/${name}"
    XDG_DIR["$name"]="${HOME_DIR[$name]}/xdg-data"
    LOG_PATH["$name"]="${TEST_ROOT}/${name}.log"

    mkdir -p "${HOME_DIR[$name]}"

    echo "[installed-native-chat] install ${name}"
    HOME="${HOME_DIR[$name]}" \
    XDG_DATA_HOME="${XDG_DIR[$name]}" \
    ELASTOS_PUBLISHER_GATEWAY="${PUBLISHER_GATEWAY}" \
    bash -lc 'curl -fsSL "${ELASTOS_PUBLISHER_GATEWAY%/}/install.sh" | bash' \
        >/tmp/elastos-installed-native-chat-install-"${name}".log 2>&1

    BIN_PATH["$name"]="${HOME_DIR[$name]}/.local/bin/elastos"
    if [[ ! -x "${BIN_PATH[$name]}" ]]; then
        echo "[installed-native-chat] installed binary missing for ${name}: ${BIN_PATH[$name]}" >&2
        exit 1
    fi

    echo "[installed-native-chat] setup ${name}"
    HOME="${HOME_DIR[$name]}" \
    XDG_DATA_HOME="${XDG_DIR[$name]}" \
    "${BIN_PATH[$name]}" setup --profile pc2 \
        >/tmp/elastos-installed-native-chat-setup-"${name}".log 2>&1
}

launch_chat() {
    local name="$1"
    local nick="$2"
    local message="$3"
    local send_delay="$4"
    local quit_delay="$5"

    (
        sleep "${send_delay}"
        echo "${message}"
        sleep "${quit_delay}"
        echo "/quit"
    ) | HOME="${HOME_DIR[$name]}" \
        XDG_DATA_HOME="${XDG_DIR[$name]}" \
        ELASTOS_CHAT_FORCE_STDIN=1 \
        "${BIN_PATH[$name]}" chat --nick "${nick}" >"${LOG_PATH[$name]}" 2>&1 &
    PID["$name"]=$!
}

wait_for_chat() {
    local name="$1"
    if ! wait "${PID[$name]}"; then
        echo "[installed-native-chat] chat exited non-zero for ${name}" >&2
        sed -n '1,240p' "${LOG_PATH[$name]}" >&2 || true
        exit 1
    fi
}

echo "[installed-native-chat] test root: ${TEST_ROOT}"
echo "[installed-native-chat] publisher gateway: ${PUBLISHER_GATEWAY}"
echo "[installed-native-chat] smoke id: ${SMOKE_ID}"

prepare_home alice
prepare_home bob

echo "[installed-native-chat] launch alice (${ALICE_NICK})"
launch_chat alice "${ALICE_NICK}" "${ALICE_MSG}" 12 10

echo "[installed-native-chat] launch bob (${BOB_NICK})"
launch_chat bob "${BOB_NICK}" "${BOB_MSG}" 18 8

wait_for_chat alice
wait_for_chat bob

assert_log_contains "${LOG_PATH[alice]}" "Chat as '${ALICE_NICK}' on #general."
assert_log_contains "${LOG_PATH[bob]}" "Chat as '${BOB_NICK}' on #general."
assert_log_contains "${LOG_PATH[alice]}" "[chat] chat room peer attached: '${BOB_NICK}' joined #general via Carrier."
assert_log_contains "${LOG_PATH[bob]}" "[chat] chat room peer attached: '${ALICE_NICK}' joined #general via Carrier."
assert_log_contains "${LOG_PATH[alice]}" "<${ALICE_NICK}> ${ALICE_MSG}"
assert_log_contains "${LOG_PATH[bob]}" "<${BOB_NICK}> ${BOB_MSG}"
assert_log_contains "${LOG_PATH[alice]}" "v <${BOB_NICK}> ${BOB_MSG}"
assert_log_contains "${LOG_PATH[bob]}" "v <${ALICE_NICK}> ${ALICE_MSG}"
assert_log_not_contains "${LOG_PATH[alice]}" "message stayed local"
assert_log_not_contains "${LOG_PATH[bob]}" "message stayed local"
assert_log_not_contains "${LOG_PATH[alice]}" "[chat] dropped unverified"
assert_log_not_contains "${LOG_PATH[bob]}" "[chat] dropped unverified"

echo "[installed-native-chat] OK"
echo "  alice log: ${LOG_PATH[alice]}"
echo "  bob log:   ${LOG_PATH[bob]}"
echo "--- alice log ---"
sed -n '1,220p' "${LOG_PATH[alice]}"
echo "--- bob log ---"
sed -n '1,220p' "${LOG_PATH[bob]}"
