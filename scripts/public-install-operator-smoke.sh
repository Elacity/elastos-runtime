#!/usr/bin/env bash
set -euo pipefail

PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"
TARGET_ADDR="${ELASTOS_OPERATOR_TARGET_ADDR:-127.0.0.1:33100}"
SRC_HOME="$(mktemp -d /tmp/elastos-public-operator-src-XXXXXX)"
DST_HOME="$(mktemp -d /tmp/elastos-public-operator-dst-XXXXXX)"

cleanup() {
    if [[ -f "${DST_HOME}/serve.pid" ]]; then
        kill "$(cat "${DST_HOME}/serve.pid")" 2>/dev/null || true
    fi
    rm -rf "${SRC_HOME}" "${DST_HOME}"
}
trap cleanup EXIT

install_operator_home() {
    local home_dir="$1"
    echo "[public-operator] install into ${home_dir}"
    HOME="${home_dir}" \
    XDG_DATA_HOME="${home_dir}/xdg-data" \
    ELASTOS_PUBLISHER_GATEWAY="${PUBLISHER_GATEWAY}" \
    bash -lc 'mkdir -p "$HOME" "$XDG_DATA_HOME" && curl -fsSL "${ELASTOS_PUBLISHER_GATEWAY%/}/install.sh" | bash' \
        >"${home_dir}/install.log"

    local bin="${home_dir}/.local/bin/elastos"
    if [[ ! -x "${bin}" ]]; then
        echo "[public-operator] installed binary missing: ${bin}" >&2
        exit 1
    fi

    echo "[public-operator] setup --profile operator in ${home_dir}"
    HOME="${home_dir}" \
    XDG_DATA_HOME="${home_dir}/xdg-data" \
    "${bin}" setup --profile operator \
        >"${home_dir}/setup.log"
}

json_field() {
    local key="$1"
    python3 -c 'import json,sys; value=json.load(sys.stdin).get(sys.argv[1], ""); print("" if value is None else value)' "${key}"
}

retry_json_command() {
    local out_var="$1"
    shift
    local result=""
    for _ in $(seq 1 15); do
        if result="$("$@" 2>/tmp/elastos-public-operator-retry.err)"; then
            printf -v "${out_var}" '%s' "${result}"
            return 0
        fi
        sleep 1
    done
    cat /tmp/elastos-public-operator-retry.err >&2 || true
    return 1
}

install_operator_home "${SRC_HOME}"
install_operator_home "${DST_HOME}"

SRC_BIN="${SRC_HOME}/.local/bin/elastos"
DST_BIN="${DST_HOME}/.local/bin/elastos"

echo "[public-operator] start target operator runtime"
HOME="${DST_HOME}" \
XDG_DATA_HOME="${DST_HOME}/xdg-data" \
bash -lc '"$HOME/.local/bin/elastos" serve --addr "'"${TARGET_ADDR}"'" >"$HOME/serve.log" 2>&1 & echo $! >"$HOME/serve.pid"'

SRC_INFO="$(
    HOME="${SRC_HOME}" \
    XDG_DATA_HOME="${SRC_HOME}/xdg-data" \
    "${SRC_BIN}" node info --json
)"
SRC_DID="$(printf '%s' "${SRC_INFO}" | json_field did)"

DST_INFO=""
DST_DID=""
DST_TICKET=""
for _ in $(seq 1 30); do
    DST_INFO="$(
        HOME="${DST_HOME}" \
        XDG_DATA_HOME="${DST_HOME}/xdg-data" \
        "${DST_BIN}" node info --json
    )"
    DST_DID="$(printf '%s' "${DST_INFO}" | json_field did)"
    DST_TICKET="$(printf '%s' "${DST_INFO}" | json_field connect_ticket)"
    if [[ -n "${DST_TICKET}" && "${DST_TICKET}" != "null" ]]; then
        break
    fi
    sleep 1
done

if [[ -z "${DST_TICKET}" || "${DST_TICKET}" == "null" ]]; then
    echo "[public-operator] target connect ticket unavailable" >&2
    exit 1
fi

echo "[public-operator] authorize source DID on target"
HOME="${DST_HOME}" \
XDG_DATA_HOME="${DST_HOME}/xdg-data" \
"${DST_BIN}" node peer add \
    --did "${SRC_DID}" \
    --allow status.read \
    --allow update.check \
    --allow update.apply \
    >"${DST_HOME}/peer-add.log"

echo "[public-operator] add target peer on source"
HOME="${SRC_HOME}" \
XDG_DATA_HOME="${SRC_HOME}/xdg-data" \
"${SRC_BIN}" node peer add \
    --did "${DST_DID}" \
    --ticket "${DST_TICKET}" \
    >"${SRC_HOME}/peer-add.log"

retry_json_command STATUS_JSON \
    env HOME="${SRC_HOME}" XDG_DATA_HOME="${SRC_HOME}/xdg-data" \
    "${SRC_BIN}" node status --peer "${DST_DID}" --json
retry_json_command UPDATE_JSON \
    env HOME="${SRC_HOME}" XDG_DATA_HOME="${SRC_HOME}/xdg-data" \
    "${SRC_BIN}" node update --peer "${DST_DID}" --check --json

STATUS_JSON="${STATUS_JSON}" UPDATE_JSON="${UPDATE_JSON}" python3 - <<'PY'
import json
import os

status = json.loads(os.environ["STATUS_JSON"])
update = json.loads(os.environ["UPDATE_JSON"])

assert status["runtime_running"] is True, status
assert status["runtime_kind"] == "operator", status
assert update["discovery"] == "Carrier", update
assert update["channel"] == "jetson-test", update
assert update["current_version"] == "0.1.2", update
assert update["latest_version"] == "0.1.2", update
assert update["update_available"] is False, update
PY

echo "[public-operator] status JSON"
echo "${STATUS_JSON}"
echo "[public-operator] update JSON"
echo "${UPDATE_JSON}"
echo "[public-operator] OK"
