#!/usr/bin/env bash
# Start one elastos gateway and require the /healthz owner check.
# docs/LAUNCHER_CONTRACT.md: HTTP 200 and the two bytes OK when this process
# owns host-process.lock with role gateway or gateway-public.
# An empty Home can pass. This script does not prove providers, capsules,
# or a signed install. elastos serve uses GET /api/health for that status.
# The product command is `elastos gateway --addr localhost:8090`. This script
# binds one numeric loopback socket so the pid check has one listener. It does
# not prove localhost name resolution or port 8090. On macOS, localhost can
# resolve to ::1 while this bind is 127.0.0.1 or [::1].
set -euo pipefail

if [[ $# -ne 1 || -z "${1:-}" ]]; then
    echo "usage: gateway-healthz-smoke.sh /path/to/elastos" >&2
    exit 2
fi

binary=$1
if [[ -z "${ELASTOS_GATEWAY_HEALTH_DEADLINE+x}" ]]; then
    deadline=120
else
    deadline=$ELASTOS_GATEWAY_HEALTH_DEADLINE
fi
if [[ ! "$deadline" =~ ^[1-9][0-9]{0,3}$ ]]; then
    echo "ELASTOS_GATEWAY_HEALTH_DEADLINE must be a positive integer" >&2
    exit 2
fi
if ! command -v curl >/dev/null 2>&1 || ! command -v lsof >/dev/null 2>&1; then
    echo "curl and lsof are required" >&2
    exit 2
fi

if [[ -z "${ELASTOS_GATEWAY_ADDR:-}" ]]; then
    port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
    addr="127.0.0.1:${port}"
else
    addr=$ELASTOS_GATEWAY_ADDR
fi
# A numeric loopback address keeps the listener check on one socket.
if [[ ! "$addr" =~ ^(127\.0\.0\.1|\[::1\]):[0-9]+$ ]]; then
    echo "gateway smoke binds a numeric loopback address: $addr" >&2
    exit 2
fi
if [[ ! -x "$binary" ]]; then
    echo "elastos binary is missing or is not executable: $binary" >&2
    exit 1
fi

root=$(mktemp -d)
pid=
log=
group_alive() {
    kill -0 -"$pid" 2>/dev/null
}
stop_gateway() {
    if [[ -z "$pid" ]]; then
        return 0
    fi
    kill -TERM -"$pid" 2>/dev/null || true
    local i
    for i in 1 2 3 4 5 6 7 8 9 10; do
        group_alive || { wait "$pid" 2>/dev/null || true; return 0; }
        sleep 0.2
    done
    kill -KILL -"$pid" 2>/dev/null || true
    for i in 1 2 3 4 5 6 7 8 9 10; do
        group_alive || { wait "$pid" 2>/dev/null || true; return 0; }
        sleep 0.2
    done
    echo "gateway process group $pid is still running" >&2
    return 1
}
cleanup() {
    local status=$?
    trap - EXIT INT TERM HUP
    stop_gateway || status=1
    if [[ -n "${ELASTOS_GATEWAY_LOG:-}" && -f "$log" ]]; then
        cp "$log" "$ELASTOS_GATEWAY_LOG" 2>/dev/null || true
    fi
    rm -rf "$root"
    exit "$status"
}
trap cleanup EXIT INT TERM HUP

export HOME="$root/home"
export XDG_DATA_HOME="$root/home/.local/share"
mkdir -p "$HOME" "$XDG_DATA_HOME"
log="$root/gateway.log"
expect="$root/healthz-expect"
body="$root/healthz-body"
printf 'OK' >"$expect"

# The new session is the process group. Cleanup signals that group.
python3 -c 'import os, sys; os.setsid(); os.execv(sys.argv[1], sys.argv[1:])' \
    "$binary" gateway --addr "$addr" >"$log" 2>&1 &
pid=$!

url="http://${addr}/healthz"
port="${addr##*:}"
end=$((SECONDS + deadline))
while true; do
    if ! kill -0 "$pid" 2>/dev/null; then
        echo "gateway exited before /healthz returned OK" >&2
        exit 1
    fi
    code=$(curl --noproxy '*' -sS --max-time 2 -o "$body" -w '%{http_code}' "$url" 2>/dev/null || true)
    if [[ "$code" == 200 ]]; then
        if cmp -s "$expect" "$body"; then
            if ! lsof -a -p "$pid" -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
                echo "GET $url responder is not gateway pid $pid" >&2
                exit 1
            fi
            echo "GET $url returned HTTP 200 and the two bytes OK from pid $pid"
            exit 0
        fi
        echo "GET $url body is not the two bytes OK" >&2
        exit 1
    elif [[ -n "$code" && "$code" != 000 ]]; then
        echo "GET $url returned HTTP $code" >&2
        exit 1
    fi
    if (( SECONDS >= end )); then
        echo "GET $url did not return OK within ${deadline}s" >&2
        exit 1
    fi
    sleep 1
done
