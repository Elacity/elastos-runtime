#!/usr/bin/env bash
# Start one elastos gateway and require the readiness that
# docs/LAUNCHER_CONTRACT.md "Readiness and Home URL" defines:
#   GET http://localhost:8090/healthz returns HTTP 200 and the two bytes OK
#   while this gateway pid holds {data}/host-process.lock with role gateway
#   or gateway-public.
# By default the gateway gets the contract command
# `elastos gateway --addr localhost:8090` and the probe uses the contract URL.
# The probe passes only when:
#   - curl gets HTTP 200 and the two bytes OK (no proxy, no redirect),
#   - the address curl connected to is a listen socket of the gateway pid,
#   - host-process.lock names that pid, a gateway role and the same addr.
# The script then prints facts, not pass or fail: the localhost lookup order,
# the addresses the gateway listens on, and what GET /healthz returns at
# localhost, 127.0.0.1 and [::1] on the same port.
# An empty Home can pass. This does not prove providers, capsules, sign-in,
# or a signed install.
set -euo pipefail

usage() {
    echo "usage: gateway-healthz-smoke.sh /path/to/elastos" >&2
    exit 2
}
[[ $# -eq 1 && -n "${1:-}" ]] || usage
binary=$1

deadline=${ELASTOS_GATEWAY_HEALTH_DEADLINE-120}
if [[ ! "$deadline" =~ ^[1-9][0-9]{0,3}$ ]]; then
    echo "ELASTOS_GATEWAY_HEALTH_DEADLINE must be a whole number of seconds from 1 to 9999" >&2
    exit 2
fi

addr=${ELASTOS_GATEWAY_ADDR-localhost:8090}
# Loopback only. localhost is the contract name; the numeric forms are for
# tests that need a free port.
if [[ ! "$addr" =~ ^(localhost|127\.0\.0\.1|\[::1\]):([1-9][0-9]{0,4})$ ]] \
    || (( BASH_REMATCH[2] > 65535 )); then
    echo "gateway smoke needs a loopback host:port (localhost, 127.0.0.1 or [::1]): $addr" >&2
    exit 2
fi
port=${BASH_REMATCH[2]}

for tool in curl lsof python3; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "gateway smoke needs $tool" >&2
        exit 2
    fi
done
case "$(uname -s)" in
    Darwin) data_subdir="Library/Application Support/elastos" ;;
    Linux) data_subdir=".local/share/elastos" ;;
    *)
        echo "gateway smoke does not know the data directory on $(uname -s)" >&2
        exit 2
        ;;
esac
if [[ ! -x "$binary" ]]; then
    echo "elastos binary is missing or is not executable: $binary" >&2
    exit 1
fi

root=$(mktemp -d)
pid=
log="$root/gateway.log"

# kill -0 succeeds for the group while any member is alive, and for the pid
# until it exits. Check both, in case the group did not form.
gateway_alive() {
    kill -0 -- "-$pid" 2>/dev/null || kill -0 "$pid" 2>/dev/null
}
signal_gateway() {
    kill "-$1" -- "-$pid" 2>/dev/null || true
    kill "-$1" "$pid" 2>/dev/null || true
}
wait_gone() {
    local i
    for i in $(seq 1 25); do
        gateway_alive || return 0
        sleep 0.2
    done
    return 1
}
stop_gateway() {
    [[ -n "$pid" ]] || return 0
    signal_gateway TERM
    wait_gone && return 0
    signal_gateway KILL
    wait_gone && return 0
    echo "gateway process group $pid is still running after SIGKILL" >&2
    return 1
}
cleanup() {
    local status=$1
    trap - EXIT INT TERM HUP
    stop_gateway || status=1
    if [[ -n "${ELASTOS_GATEWAY_LOG:-}" && -f "$log" ]]; then
        cp "$log" "$ELASTOS_GATEWAY_LOG" 2>/dev/null || true
    fi
    rm -rf "$root"
    exit "$status"
}
trap 'cleanup $?' EXIT
trap 'cleanup 130' INT
trap 'cleanup 143' TERM
trap 'cleanup 129' HUP

export HOME="$root/home"
export XDG_DATA_HOME="$HOME/.local/share"
mkdir -p "$HOME" "$XDG_DATA_HOME"
lock="$HOME/$data_subdir/host-process.lock"
expect="$root/healthz-expect"
body="$root/healthz-body"
printf 'OK' >"$expect"

# The gateway leads a new session, so its pid is also its process group id.
python3 -c 'import os, sys; os.setsid(); os.execv(sys.argv[1], sys.argv[1:])' \
    "$binary" gateway --addr "$addr" >"$log" 2>&1 &
pid=$!

url="http://${addr}/healthz"
end=$((SECONDS + deadline))
last="no answer"
while true; do
    if ! kill -0 "$pid" 2>/dev/null; then
        echo "gateway exited before GET $url was ready" >&2
        exit 1
    fi
    : >"$body"
    answer=$(curl --noproxy '*' --proto =http -sS --max-time 2 -o "$body" \
        -w '%{http_code} %{remote_ip}' "$url" 2>/dev/null || true)
    code=${answer%% *}
    remote_ip=${answer#* }
    case "$code" in
        200) break ;;
        503) last="HTTP 503" ;;
        "" | 000) last="no answer" ;;
        *)
            echo "GET $url returned HTTP $code. Only 200 (ready) or 503 (not ready) is allowed." >&2
            exit 1
            ;;
    esac
    if (( SECONDS >= end )); then
        echo "GET $url was not ready within ${deadline}s (last: $last)" >&2
        exit 1
    fi
    sleep 1
done

if ! cmp -s "$expect" "$body"; then
    echo "GET $url returned HTTP 200, but the body is not the two bytes OK" >&2
    exit 1
fi
case "$remote_ip" in
    127.0.0.1) socket="127.0.0.1" ;;
    ::1) socket="[::1]" ;;
    *)
        echo "GET $url connected to '$remote_ip', which is not a loopback address" >&2
        exit 1
        ;;
esac
if ! lsof -nP -a -p "$pid" -iTCP@"$socket":"$port" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "GET $url was answered on $socket:$port, which is not a listen socket of gateway pid $pid" >&2
    exit 1
fi
if ! python3 - "$lock" "$pid" "$addr" <<'PY'
import json, sys
path, pid, addr = sys.argv[1], int(sys.argv[2]), sys.argv[3]
try:
    with open(path, "rb") as handle:
        meta = json.load(handle)
except (OSError, ValueError) as error:
    sys.exit(f"host-process.lock is missing or is not JSON: {error}")
if not isinstance(meta, dict):
    sys.exit("host-process.lock is not a JSON object")
if meta.get("pid") != pid:
    sys.exit(f"host-process.lock names pid {meta.get('pid')!r}, not gateway pid {pid}")
if meta.get("role") not in ("gateway", "gateway-public"):
    sys.exit(f"host-process.lock role is {meta.get('role')!r}, not gateway")
if meta.get("addr") != addr:
    sys.exit(f"host-process.lock addr is {meta.get('addr')!r}, not {addr}")
PY
then
    exit 1
fi

echo "ready: GET $url returned HTTP 200 and OK from gateway pid $pid on $socket:$port"
echo "ready: host-process.lock names pid $pid with role gateway and addr $addr"

# Facts only, not pass or fail: what each loopback spelling of this port
# reaches. A client that uses a different spelling than the gateway's --addr
# can miss the gateway or reach another process.
listening=$(lsof -nP -a -p "$pid" -iTCP:"$port" -sTCP:LISTEN 2>/dev/null \
    | awk 'NR > 1 { print $(NF - 1) }' | sort -u | paste -sd ' ' - || true)
echo "fact: gateway pid $pid listens on: ${listening:-nothing}"
resolved=$(python3 -c 'import socket, sys
seen = []
for info in socket.getaddrinfo("localhost", int(sys.argv[1]), type=socket.SOCK_STREAM):
    if info[4][0] not in seen:
        seen.append(info[4][0])
print(" ".join(seen))' "$port" 2>/dev/null || echo "lookup failed")
echo "fact: this host resolves localhost to (in order): $resolved"
for probe in "localhost:$port" "127.0.0.1:$port" "[::1]:$port"; do
    : >"$body"
    answer=$(curl --noproxy '*' --proto =http -sS --max-time 2 -o "$body" \
        -w '%{http_code} %{remote_ip}' "http://$probe/healthz" 2>/dev/null || true)
    code=${answer%% *}
    ip=${answer#* }
    if [[ -z "$code" || "$code" == 000 ]]; then
        echo "fact: GET http://$probe/healthz: no answer"
        continue
    fi
    case "$ip" in ::1) ip="[::1]" ;; esac
    owner="another process"
    if lsof -nP -a -p "$pid" -iTCP@"$ip":"$port" -sTCP:LISTEN >/dev/null 2>&1; then
        owner="gateway pid $pid"
    fi
    shown="not OK"
    cmp -s "$expect" "$body" && shown="OK"
    echo "fact: GET http://$probe/healthz: HTTP $code, body $shown, from $owner on $ip:$port"
done
