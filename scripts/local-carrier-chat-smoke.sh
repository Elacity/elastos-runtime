#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_ELASTOS_BIN="${ROOT}/elastos/target/debug/elastos"
ELASTOS_BIN="${ELASTOS_BIN:-${DEFAULT_ELASTOS_BIN}}"
HOST_DATA_DIR="${ELASTOS_HOST_DATA_DIR:-}"
TEST_ROOT="${ELASTOS_LOCAL_CHAT_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/elastos-local-chat.XXXXXX")}"
SKIP_BUILD=0
TOPIC="${ELASTOS_CHAT_TOPIC:-#general}"
BOOTSTRAP_MODE="${ELASTOS_CHAT_BOOTSTRAP_MODE:-direct}"
DISCOVERY_TOPIC="__elastos_internal/chat-presence-v1/${TOPIC}"
PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"
MAINTAINER_DID="${ELASTOS_MAINTAINER_DID:-did:key:z6Mkf2nCJ1pcN4JioAxHEiyDsPC298QFtn2Dgg9tjt2ezHeK}"
SOURCE_RUNTIME_COORDS="${ELASTOS_SOURCE_RUNTIME_COORDS:-$HOME/.local/share/elastos/runtime-coords.json}"
SOURCE_TICKET_OVERRIDE="${ELASTOS_CHAT_SOURCE_TICKET:-}"
BOOTSTRAP_HOME=""

declare -A HOME_DIR XDG_DIR DATA_DIR LOG_PATH COORDS_PATH API_URL ATTACH_SECRET TOKEN CAP_TOKEN PID

usage() {
    cat <<'EOF'
Usage:
  bash scripts/local-carrier-chat-smoke.sh
  bash scripts/local-carrier-chat-smoke.sh --skip-build

What it proves:
  1. Starts three local ElastOS runtimes: seed, alice, bob
  2. Attaches to each runtime with real session tokens
  3. Requests real Carrier peer capability tokens
  4. Alice and Bob connect to seed over Carrier
  5. They join the same topic using the configured bootstrap mode
  6. Alice sends a message and Bob receives it
  7. Bob replies and Alice receives it

Modes:
  - `direct`: legacy seed-participates-in-room flow
  - `source-dht`: experimental external DHT discovery
  - `source-rendezvous`: Carrier-only automatic peer rendezvous through the
    trusted source, then direct room joins between discovered peers

If `ELASTOS_CHAT_SOURCE_TICKET` is set, the script will not start a local seed.
Instead alice and bob will use that live trusted-source ticket.

This uses the current source-local elastos binary by default. By default it
also provisions a fresh coherent install root inside the temp test directory
so provider binaries and components.json always match. Set ELASTOS_HOST_DATA_DIR
to override that and reuse an existing installed data dir.
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

if [[ ! -x "${ELASTOS_BIN}" ]]; then
    echo "Missing source-local elastos binary: ${ELASTOS_BIN}" >&2
    echo "Run: cargo build --manifest-path ${ROOT}/elastos/Cargo.toml -p elastos-server" >&2
    exit 1
fi

case "${BOOTSTRAP_MODE}" in
    source-dht|source-rendezvous|direct) ;;
    *)
        echo "Unsupported ELASTOS_CHAT_BOOTSTRAP_MODE: ${BOOTSTRAP_MODE}" >&2
        echo "Use 'direct', 'source-dht', or 'source-rendezvous'." >&2
        exit 1
        ;;
esac

cleanup() {
    for name in "${!PID[@]}"; do
        if [[ -n "${PID[$name]:-}" ]] && kill -0 "${PID[$name]}" 2>/dev/null; then
            kill "${PID[$name]}" 2>/dev/null || true
            wait "${PID[$name]}" 2>/dev/null || true
        fi
    done
}
trap cleanup EXIT

prepare_host_data_dir() {
    if [[ -n "${HOST_DATA_DIR}" ]]; then
        return 0
    fi

    if [[ ! -f "${SOURCE_RUNTIME_COORDS}" ]]; then
        echo "Missing source runtime coords for coherent install root: ${SOURCE_RUNTIME_COORDS}" >&2
        return 1
    fi

    mapfile -t source_bootstrap < <(RUNTIME_COORDS="${SOURCE_RUNTIME_COORDS}" python3 - <<'PY'
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
    )
    local source_connect_ticket="${source_bootstrap[0]:-}"
    local source_node_id="${source_bootstrap[1]:-}"
    if [[ -z "${source_connect_ticket}" || -z "${source_node_id}" ]]; then
        echo "Failed to discover live trusted-source Carrier bootstrap" >&2
        return 1
    fi

    BOOTSTRAP_HOME="${TEST_ROOT}/install-root"
    mkdir -p "${BOOTSTRAP_HOME}"
    echo "[local-carrier-chat] prepare coherent install root"
    HOME="${BOOTSTRAP_HOME}" \
    XDG_DATA_HOME="${BOOTSTRAP_HOME}/xdg-data" \
    ELASTOS_PUBLISHER_GATEWAY="${PUBLISHER_GATEWAY}" \
    ELASTOS_MAINTAINER_DID="${MAINTAINER_DID}" \
    ELASTOS_SOURCE_CONNECT_TICKET="${source_connect_ticket}" \
    ELASTOS_PUBLISHER_NODE_ID="${source_node_id}" \
    bash "${ROOT}/scripts/install.sh" >/tmp/elastos-local-chat-install.log

    cp "${ROOT}/components.json" "${BOOTSTRAP_HOME}/xdg-data/elastos/components.json"
    HOME="${BOOTSTRAP_HOME}" \
    XDG_DATA_HOME="${BOOTSTRAP_HOME}/xdg-data" \
    "${ELASTOS_BIN}" setup --with shell --with localhost-provider --with did-provider \
        >/tmp/elastos-local-chat-setup.log

    HOST_DATA_DIR="${BOOTSTRAP_HOME}/xdg-data/elastos"
}

free_port() {
    python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

json_post() {
    local url="$1"
    local auth_token="$2"
    local cap_token="$3"
    local body="$4"
    local -a headers=(-H "Content-Type: application/json")
    if [[ -n "${auth_token}" ]]; then
        headers+=(-H "Authorization: Bearer ${auth_token}")
    fi
    if [[ -n "${cap_token}" ]]; then
        headers+=(-H "X-Capability-Token: ${cap_token}")
    fi
    curl -sS -X POST "${url}" "${headers[@]}" -d "${body}"
}

json_get() {
    local url="$1"
    local auth_token="$2"
    local -a headers=()
    if [[ -n "${auth_token}" ]]; then
        headers+=(-H "Authorization: Bearer ${auth_token}")
    fi
    curl -sS "${headers[@]}" "${url}"
}

wait_for_file() {
    local path="$1"
    for _ in $(seq 1 60); do
        [[ -f "${path}" ]] && return 0
        sleep 0.25
    done
    return 1
}

wait_for_health() {
    local api="$1"
    for _ in $(seq 1 60); do
        if [[ "$(curl -s -o /dev/null -w '%{http_code}' "${api}/api/health" || true)" == "200" ]]; then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

start_runtime() {
    local name="$1"
    local port
    port="$(free_port)"
    HOME_DIR["$name"]="${TEST_ROOT}/${name}"
    XDG_DIR["$name"]="${HOME_DIR[$name]}/xdg-data"
    DATA_DIR["$name"]="${XDG_DIR[$name]}/elastos"
    LOG_PATH["$name"]="${TEST_ROOT}/${name}.log"
    COORDS_PATH["$name"]="${DATA_DIR[$name]}/runtime-coords.json"

    mkdir -p "${DATA_DIR[$name]}/bin"
    cp "${HOST_DATA_DIR}/components.json" "${DATA_DIR[$name]}/components.json"
    cp -a "${HOST_DATA_DIR}/bin/." "${DATA_DIR[$name]}/bin/"

    HOME="${HOME_DIR[$name]}" \
    XDG_DATA_HOME="${XDG_DIR[$name]}" \
    ELASTOS_DATA_DIR="${DATA_DIR[$name]}" \
    "${ELASTOS_BIN}" serve --addr "127.0.0.1:${port}" >"${LOG_PATH[$name]}" 2>&1 &
    PID["$name"]=$!

    wait_for_file "${COORDS_PATH[$name]}" || {
        echo "runtime-coords.json missing for ${name}. See ${LOG_PATH[$name]}" >&2
        return 1
    }

    API_URL["$name"]="$(jq -r '.api_url' "${COORDS_PATH[$name]}")"
    ATTACH_SECRET["$name"]="$(jq -r '.attach_secret' "${COORDS_PATH[$name]}")"

    wait_for_health "${API_URL[$name]}" || {
        echo "runtime health never went ready for ${name}. See ${LOG_PATH[$name]}" >&2
        return 1
    }
}

attach_runtime() {
    local name="$1"
    TOKEN["$name"]="$(json_post \
        "${API_URL[$name]}/api/auth/attach" \
        "" \
        "" \
        "$(jq -nc --arg secret "${ATTACH_SECRET[$name]}" '{secret:$secret,scope:"shell"}')" \
        | jq -r '.token')"
    [[ -n "${TOKEN[$name]}" && "${TOKEN[$name]}" != "null" ]]
}

request_capability() {
    local name="$1"
    local resp token request_id status
    resp="$(json_post \
        "${API_URL[$name]}/api/capability/request" \
        "${TOKEN[$name]}" \
        "" \
        '{"resource":"elastos://peer/*","action":"execute"}')"
    token="$(jq -r '.token // empty' <<<"${resp}")"
    if [[ -n "${token}" ]]; then
        CAP_TOKEN["$name"]="${token}"
        return 0
    fi
    request_id="$(jq -r '.request_id // empty' <<<"${resp}")"
    [[ -n "${request_id}" ]] || {
        echo "capability request failed for ${name}: ${resp}" >&2
        return 1
    }
    for _ in $(seq 1 60); do
        status="$(json_get "${API_URL[$name]}/api/capability/request/${request_id}" "${TOKEN[$name]}")"
        token="$(jq -r '.token // empty' <<<"${status}")"
        if [[ -n "${token}" ]]; then
            CAP_TOKEN["$name"]="${token}"
            return 0
        fi
        case "$(jq -r '.status // empty' <<<"${status}")" in
            denied|expired)
                echo "capability request ${request_id} ${name}: ${status}" >&2
                return 1
                ;;
        esac
        sleep 0.1
    done
    echo "capability request timed out for ${name}" >&2
    return 1
}

peer_post() {
    local name="$1"
    local op="$2"
    local body="$3"
    json_post \
        "${API_URL[$name]}/api/provider/peer/${op}" \
        "${TOKEN[$name]}" \
        "${CAP_TOKEN[$name]}" \
        "${body}"
}

get_ticket() {
    local name="$1"
    peer_post "${name}" "get_ticket" '{}' | jq -r '.data.ticket'
}

get_node_id() {
    local name="$1"
    peer_post "${name}" "get_node_id" '{}' | jq -r '.data.node_id'
}

announce_presence() {
    local name="$1"
    local did="$2"
    local nick="$3"
    local ticket="$4"
    local payload
    payload="$(jq -nc \
        --arg kind "chat_presence_v1" \
        --arg room "${TOPIC}" \
        --arg did "${did}" \
        --arg nick "${nick}" \
        --arg ticket "${ticket}" \
        '{kind:$kind,room:$room,did:$did,nick:$nick,ticket:$ticket,ts:0}')"
    peer_post "${name}" "gossip_send" "$(jq -nc \
        --arg topic "${DISCOVERY_TOPIC}" \
        --arg sender "${nick}" \
        --arg sender_id "${did}" \
        --arg message "${payload}" \
        '{topic:$topic,sender:$sender,sender_id:$sender_id,message:$message}')"
}

wait_for_presence() {
    local name="$1"
    local consumer_id="$2"
    local expected_did="$3"
    local resp
    for _ in $(seq 1 80); do
        resp="$(peer_post "${name}" "gossip_recv" "$(jq -nc \
            --arg topic "${DISCOVERY_TOPIC}" \
            --arg consumer "${consumer_id}" \
            '{topic:$topic,limit:20,consumer_id:$consumer}')")"
        if jq -e --arg did "${expected_did}" '
            .data.messages[]?
            | .content
            | fromjson?
            | select(.kind == "chat_presence_v1" and .did == $did)
        ' >/dev/null <<<"${resp}"; then
            echo "${resp}"
            return 0
        fi
        sleep 0.25
    done
    echo "presence not observed on ${name}: ${expected_did}" >&2
    return 1
}

wait_for_mutual_presence() {
    local alice_did="$1"
    local alice_nick="$2"
    local alice_ticket="$3"
    local bob_did="$4"
    local bob_nick="$5"
    local bob_ticket="$6"
    for _ in $(seq 1 24); do
        announce_presence alice "${alice_did}" "${alice_nick}" "${alice_ticket}" >/dev/null
        announce_presence bob "${bob_did}" "${bob_nick}" "${bob_ticket}" >/dev/null
        if wait_for_presence alice "alice-presence" "${bob_did}" 2>/dev/null \
            && wait_for_presence bob "bob-presence" "${alice_did}" 2>/dev/null; then
            return 0
        fi
        sleep 0.5
    done
    echo "mutual presence rendezvous did not converge" >&2
    wait_for_presence alice "alice-presence-final" "${bob_did}"
    wait_for_presence bob "bob-presence-final" "${alice_did}"
}

join_topic_peers_from_connect() {
    local name="$1"
    local connect_json="$2"
    local peers_json
    peers_json="$(jq -c '
        (.data.connected // [])
        | if length > 0 then . else (.data.added // []) end
    ' <<<"${connect_json}")"
    if [[ "${peers_json}" == "[]" ]]; then
        echo "no Carrier peer ids available for ${name} join_peers" >&2
        return 1
    fi
    peer_post "${name}" "gossip_join_peers" "$(jq -nc \
        --arg topic "${TOPIC}" \
        --argjson peers "${peers_json}" \
        '{topic:$topic,peers:$peers}')"
}

remember_topic_peer() {
    local name="$1"
    local ticket="$2"
    peer_post "${name}" "remember_peer" "$(jq -nc --arg ticket "${ticket}" '{ticket:$ticket}')"
}

wait_for_connected_peers() {
    local name="$1"
    local min_count="$2"
    local count=""
    for _ in $(seq 1 80); do
        count="$(peer_post "${name}" "list_peers" '{}' | jq -r '.data.peers | length')"
        if [[ "${count}" =~ ^[0-9]+$ ]] && (( count >= min_count )); then
            return 0
        fi
        sleep 0.25
    done
    echo "peer count for ${name} stayed below ${min_count}" >&2
    peer_post "${name}" "list_peers" '{}' >&2 || true
    return 1
}

topic_has_peer() {
    local name="$1"
    local topic="$2"
    local expected_peer="$3"
    local resp
    resp="$(peer_post "${name}" "list_topic_peers" "$(jq -nc --arg topic "${topic}" '{topic:$topic}')")"
    jq -e --arg peer "${expected_peer}" '.data.peers[]? | select(. == $peer)' >/dev/null <<<"${resp}"
}

wait_for_topic_peer() {
    local name="$1"
    local topic="$2"
    local expected_peer="$3"
    local attempts="${4:-80}"
    for _ in $(seq 1 "${attempts}"); do
        if topic_has_peer "${name}" "${topic}" "${expected_peer}"; then
            return 0
        fi
        sleep 0.25
    done
    echo "topic peer not observed on ${name}: ${expected_peer} in ${topic}" >&2
    peer_post "${name}" "list_topic_peers" "$(jq -nc --arg topic "${topic}" '{topic:$topic}')" >&2 || true
    return 1
}

attach_topic_peer() {
    local name="$1"
    local peer_ids_json="$2"
    local expected_peer="$3"
    for _ in $(seq 1 24); do
        peer_post "${name}" "gossip_join_peers" "$(jq -nc \
            --arg topic "${TOPIC}" \
            --argjson peers "${peer_ids_json}" \
            '{topic:$topic,peers:$peers}')" >/dev/null || true
        if topic_has_peer "${name}" "${TOPIC}" "${expected_peer}"; then
            return 0
        fi
        sleep 0.15
    done
    wait_for_topic_peer "${name}" "${TOPIC}" "${expected_peer}" 8
}

wait_for_message() {
    local name="$1"
    local consumer_id="$2"
    local expected_sender="$3"
    local expected_content="$4"
    local resp
    for _ in $(seq 1 80); do
        resp="$(peer_post "${name}" "gossip_recv" "$(jq -nc \
            --arg topic "${TOPIC}" \
            --arg consumer "${consumer_id}" \
            '{topic:$topic,limit:20,consumer_id:$consumer}')")"
        if jq -e \
            --arg sender "${expected_sender}" \
            --arg content "${expected_content}" \
            '.data.messages[]? | select(.sender_nick == $sender and .content == $content)' \
            >/dev/null <<<"${resp}"; then
            return 0
        fi
        sleep 0.25
    done
    echo "message not observed on ${name}: ${expected_sender}: ${expected_content}" >&2
    peer_post "${name}" "gossip_recv" "$(jq -nc \
        --arg topic "${TOPIC}" \
        --arg consumer "${consumer_id}" \
        '{topic:$topic,limit:20,consumer_id:$consumer}')" >&2 || true
    return 1
}

dump_logs() {
    for name in seed alice bob; do
        local path="${LOG_PATH[$name]:-}"
        if [[ -z "${path}" ]]; then
            continue
        fi
        echo "--- ${name} log: ${path} ---" >&2
        tail -n 120 "${path}" >&2 || true
    done
}

if [[ "${SKIP_BUILD}" -eq 0 && "${ELASTOS_BIN}" == "${DEFAULT_ELASTOS_BIN}" ]]; then
    echo "[local-carrier-chat] build elastos binary"
    cargo build -q --manifest-path "${ROOT}/elastos/Cargo.toml" -p elastos-server
fi

echo "[local-carrier-chat] test root: ${TEST_ROOT}"
echo "[local-carrier-chat] bootstrap mode: ${BOOTSTRAP_MODE}"

prepare_host_data_dir

if [[ ! -f "${HOST_DATA_DIR}/components.json" ]]; then
    echo "Missing host components.json: ${HOST_DATA_DIR}/components.json" >&2
    exit 1
fi

if [[ ! -d "${HOST_DATA_DIR}/bin" ]]; then
    echo "Missing host installed binaries: ${HOST_DATA_DIR}/bin" >&2
    exit 1
fi

JOIN_MODE="${BOOTSTRAP_MODE}"
if [[ "${JOIN_MODE}" == "source-dht" ]]; then
    JOIN_MODE="dht"
elif [[ "${JOIN_MODE}" == "source-rendezvous" ]]; then
    JOIN_MODE="direct"
fi

RUNTIMES=(alice bob)
if [[ -z "${SOURCE_TICKET_OVERRIDE}" ]]; then
    RUNTIMES=(seed alice bob)
fi

for name in "${RUNTIMES[@]}"; do
    echo "[local-carrier-chat] start ${name}"
    start_runtime "${name}" || {
        dump_logs
        exit 1
    }
    attach_runtime "${name}" || {
        dump_logs
        exit 1
    }
    request_capability "${name}" || {
        dump_logs
        exit 1
    }
done

if [[ -n "${SOURCE_TICKET_OVERRIDE}" ]]; then
    SEED_TICKET="${SOURCE_TICKET_OVERRIDE}"
else
    SEED_TICKET="$(get_ticket seed)"
    [[ -n "${SEED_TICKET}" && "${SEED_TICKET}" != "null" ]] || {
        echo "failed to fetch seed ticket" >&2
        dump_logs
        exit 1
    }
fi

if [[ "${BOOTSTRAP_MODE}" == "direct" && -z "${SOURCE_TICKET_OVERRIDE}" ]]; then
    peer_post seed gossip_join "$(jq -nc --arg topic "${TOPIC}" '{topic:$topic,mode:"direct"}')" >/dev/null
fi
if [[ "${BOOTSTRAP_MODE}" == "source-rendezvous" && -z "${SOURCE_TICKET_OVERRIDE}" ]]; then
    peer_post seed gossip_join "$(jq -nc --arg topic "${DISCOVERY_TOPIC}" '{topic:$topic,mode:"direct"}')" >/dev/null
fi
peer_post alice connect "$(jq -nc --arg ticket "${SEED_TICKET}" '{ticket:$ticket}')" >/tmp/alice-connect.json
peer_post bob connect "$(jq -nc --arg ticket "${SEED_TICKET}" '{ticket:$ticket}')" >/tmp/bob-connect.json

wait_for_connected_peers alice 1 || {
    dump_logs
    exit 1
}
wait_for_connected_peers bob 1 || {
    dump_logs
    exit 1
}

peer_post alice gossip_join "$(jq -nc --arg topic "${TOPIC}" --arg mode "${JOIN_MODE}" '{topic:$topic,mode:$mode}')" >/dev/null
peer_post bob gossip_join "$(jq -nc --arg topic "${TOPIC}" --arg mode "${JOIN_MODE}" '{topic:$topic,mode:$mode}')" >/dev/null

if [[ "${BOOTSTRAP_MODE}" == "source-rendezvous" ]]; then
    peer_post alice gossip_join "$(jq -nc --arg topic "${DISCOVERY_TOPIC}" '{topic:$topic,mode:"direct"}')" >/dev/null
    peer_post bob gossip_join "$(jq -nc --arg topic "${DISCOVERY_TOPIC}" '{topic:$topic,mode:"direct"}')" >/dev/null

    ALICE_TICKET="$(get_ticket alice)"
    BOB_TICKET="$(get_ticket bob)"
    ALICE_DID="$(get_node_id alice)"
    BOB_DID="$(get_node_id bob)"

    wait_for_mutual_presence "${ALICE_DID}" "alice" "${ALICE_TICKET}" "${BOB_DID}" "bob" "${BOB_TICKET}" >/dev/null || {
        dump_logs
        exit 1
    }

    ALICE_REMEMBER_BOB="$(remember_topic_peer alice "${BOB_TICKET}")"
    BOB_REMEMBER_ALICE="$(remember_topic_peer bob "${ALICE_TICKET}")"
    ALICE_PEERS_JSON="$(jq -c '.data.added // []' <<<"${ALICE_REMEMBER_BOB}")"
    BOB_PEERS_JSON="$(jq -c '.data.added // []' <<<"${BOB_REMEMBER_ALICE}")"
    if [[ "${ALICE_PEERS_JSON}" == "[]" || "${BOB_PEERS_JSON}" == "[]" ]]; then
        echo "remember_peer did not return peer ids for rendezvous attach" >&2
        dump_logs
        exit 1
    fi

    attach_topic_peer alice "${ALICE_PEERS_JSON}" "$(jq -r '.[0]' <<<"${ALICE_PEERS_JSON}")" || {
        dump_logs
        exit 1
    }
    attach_topic_peer bob "${BOB_PEERS_JSON}" "$(jq -r '.[0]' <<<"${BOB_PEERS_JSON}")" || {
        dump_logs
        exit 1
    }
fi

ALICE_SEND="$(peer_post alice gossip_send "$(jq -nc \
    --arg topic "${TOPIC}" \
    --arg sender "alice" \
    --arg message "hello-from-alice" \
    '{topic:$topic,sender:$sender,message:$message}')")"
if [[ "$(jq -r '.broadcast // "ok"' <<<"${ALICE_SEND}")" == "local_only" ]]; then
    echo "alice broadcast stayed local" >&2
    dump_logs
    exit 1
fi

wait_for_message bob "bob-smoke" "alice" "hello-from-alice" || {
    dump_logs
    exit 1
}

BOB_SEND="$(peer_post bob gossip_send "$(jq -nc \
    --arg topic "${TOPIC}" \
    --arg sender "bob" \
    --arg message "hello-from-bob" \
    '{topic:$topic,sender:$sender,message:$message}')")"
if [[ "$(jq -r '.broadcast // "ok"' <<<"${BOB_SEND}")" == "local_only" ]]; then
    echo "bob broadcast stayed local" >&2
    dump_logs
    exit 1
fi

wait_for_message alice "alice-smoke" "bob" "hello-from-bob" || {
    dump_logs
    exit 1
}

echo "[local-carrier-chat] OK"
if [[ -n "${SOURCE_TICKET_OVERRIDE}" ]]; then
    echo "  seed peers:  [remote source ticket]"
else
    echo "  seed peers:  $(peer_post seed list_peers '{}' | jq -c '.data.peers')"
fi
echo "  alice peers: $(peer_post alice list_peers '{}' | jq -c '.data.peers')"
echo "  bob peers:   $(peer_post bob list_peers '{}' | jq -c '.data.peers')"
echo "  logs:"
for name in "${RUNTIMES[@]}"; do
    echo "    ${name}: ${LOG_PATH[$name]}"
done
