#!/usr/bin/env bash
# Start one elastos gateway and require the launcher readiness probe.
# The probe is GET /healthz with body OK. See docs/LAUNCHER_CONTRACT.md.
set -euo pipefail

if [[ $# -ne 1 || -z "${1:-}" ]]; then
    echo "usage: gateway-healthz-smoke.sh /path/to/elastos" >&2
    exit 2
fi

binary=$1
addr=${ELASTOS_GATEWAY_ADDR:-127.0.0.1:8090}
deadline=${ELASTOS_GATEWAY_HEALTH_DEADLINE:-120}

case "$addr" in
    127.0.0.1:*|localhost:*) ;;
    *)
        echo "gateway smoke binds loopback only: $addr" >&2
        exit 2
        ;;
esac
if [[ ! -x "$binary" ]]; then
    echo "elastos binary is missing or is not executable: $binary" >&2
    exit 1
fi

root=$(mktemp -d)
pid=
log=
stop_gateway() {
    if [[ -z "$pid" ]]; then
        return 0
    fi
    kill -TERM "$pid" 2>/dev/null || true
    local i
    for i in 1 2 3 4 5 6 7 8 9 10; do
        kill -0 "$pid" 2>/dev/null || return 0
        sleep 0.2
    done
    kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
}
cleanup() {
    local status=$?
    stop_gateway
    if [[ -n "${ELASTOS_GATEWAY_LOG:-}" && -f "$log" ]]; then
        cp "$log" "$ELASTOS_GATEWAY_LOG" 2>/dev/null || true
    fi
    rm -rf "$root"
    exit "$status"
}
trap cleanup EXIT

export HOME="$root/home"
export XDG_DATA_HOME="$root/home/.local/share"
mkdir -p "$HOME" "$XDG_DATA_HOME"
log="$root/gateway.log"

"$binary" gateway --addr "$addr" >"$log" 2>&1 &
pid=$!

url="http://${addr}/healthz"
end=$((SECONDS + deadline))
while true; do
    if ! kill -0 "$pid" 2>/dev/null; then
        echo "gateway exited before /healthz returned OK" >&2
        cat "$log" >&2 || true
        exit 1
    fi
    if body=$(curl -fsS --max-time 2 "$url" 2>/dev/null); then
        if [[ "$body" == "OK" ]]; then
            exit 0
        fi
        echo "GET $url body is not OK" >&2
        printf '%s\n' "$body" >&2
        exit 1
    fi
    if (( SECONDS >= end )); then
        echo "GET $url did not return OK within ${deadline}s" >&2
        cat "$log" >&2 || true
        exit 1
    fi
    sleep 1
done
