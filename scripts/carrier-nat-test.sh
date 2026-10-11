#!/usr/bin/env bash
# Issue #345: two Homes, each behind its own NAT (Linux network namespaces with
# MASQUERADE), reach each other by node ID alone through the seed's relay and
# exchange Chat messages over Carrier gossip in both directions.
#
# Usage (Linux, root): scripts/carrier-nat-test.sh ELASTOS_SERVER_LIB_TEST_BINARY
# Build the binary with:
#   cargo test --locked --manifest-path elastos/Cargo.toml -p elastos-server --lib --no-run
#
# Topology:
#   wan     203.0.113.1    the relay (the seed's relay role, plain HTTP)
#   nat-a   203.0.113.11 / 10.0.1.1   MASQUERADE, no inbound except replies
#   home-a  10.0.1.2
#   nat-b   203.0.113.12 / 10.0.2.1
#   home-b  10.0.2.2
set -euo pipefail

die() { echo "carrier-nat-test: $*" >&2; exit 2; }
[[ $# == 1 && -x "$1" ]] || die "usage: carrier-nat-test.sh ELASTOS_SERVER_LIB_TEST_BINARY"
[[ "$(uname -s)" == Linux && "$(id -u)" == 0 ]] || die "needs Linux and root (network namespaces)"
for tool in ip iptables ping curl; do
    command -v "$tool" >/dev/null || die "missing tool: $tool"
done

TEST_BIN="$(realpath "$1")"
RUN="elastos-nat-$$"
WORK="$(mktemp -d)"
NAMESPACES=(wan nat-a nat-b home-a home-b)
RELAY_URL=http://203.0.113.1:3340
PIDS=()

cleanup() {
    local pid ns
    for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
    for ns in "${NAMESPACES[@]}"; do ip netns del "$RUN-$ns" 2>/dev/null || true; done
    rm -rf "$WORK"
}
trap cleanup EXIT
in_ns() { local ns="$1"; shift; ip netns exec "$RUN-$ns" "$@"; }

for ns in "${NAMESPACES[@]}"; do
    ip netns add "$RUN-$ns"
    in_ns "$ns" ip link set lo up
done
in_ns wan ip link add br0 type bridge
in_ns wan ip addr add 203.0.113.1/24 dev br0
in_ns wan ip link set br0 up

index=0
for side in a b; do
    index=$((index + 1))
    ip link add up netns "$RUN-nat-$side" type veth peer name "port-$side" netns "$RUN-wan"
    in_ns wan ip link set "port-$side" master br0 up
    in_ns "nat-$side" ip addr add "203.0.113.1$index/24" dev up
    in_ns "nat-$side" ip link set up up
    ip link add lan netns "$RUN-nat-$side" type veth peer name eth0 netns "$RUN-home-$side"
    in_ns "nat-$side" ip addr add "10.0.$index.1/24" dev lan
    in_ns "nat-$side" ip link set lan up
    in_ns "home-$side" ip addr add "10.0.$index.2/24" dev eth0
    in_ns "home-$side" ip link set eth0 up
    in_ns "home-$side" ip route add default via "10.0.$index.1"
    in_ns "nat-$side" sysctl -qw net.ipv4.ip_forward=1
    in_ns "nat-$side" iptables -t nat -A POSTROUTING -o up -j MASQUERADE
    in_ns "nat-$side" iptables -P FORWARD DROP
    in_ns "nat-$side" iptables -A FORWARD -i lan -o up -j ACCEPT
    in_ns "nat-$side" iptables -A FORWARD -i up -o lan -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
done

# Each Home reaches the internet but neither Home reaches the other directly.
in_ns home-a ping -c 1 -W 2 203.0.113.1 >/dev/null || die "home-a cannot reach the internet"
in_ns home-b ping -c 1 -W 2 203.0.113.1 >/dev/null || die "home-b cannot reach the internet"
! in_ns home-a ping -c 1 -W 1 10.0.2.2 >/dev/null 2>&1 || die "home-a reaches home-b without NAT"
! in_ns home-b ping -c 1 -W 1 10.0.1.2 >/dev/null 2>&1 || die "home-b reaches home-a without NAT"
! in_ns home-a ping -c 1 -W 1 203.0.113.12 >/dev/null 2>&1 || die "nat-b answers unsolicited traffic"
echo "carrier-nat-test: two Homes behind separate NATs; no direct route between them"

run_test() {
    local ns="$1" name="$2" log="$3"
    shift 3
    in_ns "$ns" env -i PATH="$PATH" HOME="$WORK/$name-home" ELASTOS_NAT_DIR="$WORK" \
        ELASTOS_CARRIER_MDNS=0 "$@" "$TEST_BIN" --exact "carrier::relay::tests::$name" \
        --ignored --nocapture --test-threads 1 >"$log" 2>&1
}

run_test wan nat_relay "$WORK/relay.log" ELASTOS_NAT_RELAY_BIND=203.0.113.1:3340 &
PIDS+=($!)
relay_pid=$!
for _ in $(seq 1 120); do
    [[ -e "$WORK/relay.ready" ]] && break
    kill -0 "$relay_pid" 2>/dev/null || { cat "$WORK/relay.log"; die "relay exited"; }
    sleep 0.5
done
[[ -e "$WORK/relay.ready" ]] || { cat "$WORK/relay.log"; die "relay did not start"; }
in_ns home-a curl -fsS -m 5 -o /dev/null "$RELAY_URL/" || die "home-a cannot reach the relay through its NAT"

run_test home-a nat_home "$WORK/home-a.log" ELASTOS_NAT_NAME=a ELASTOS_NAT_PEER=b ELASTOS_NAT_RELAY_URL="$RELAY_URL" &
PIDS+=($!)
home_a=$!
run_test home-b nat_home "$WORK/home-b.log" ELASTOS_NAT_NAME=b ELASTOS_NAT_PEER=a ELASTOS_NAT_RELAY_URL="$RELAY_URL" &
PIDS+=($!)
home_b=$!

status=0
for pid in "$home_a" "$home_b" "$relay_pid"; do
    wait "$pid" || status=1
done
PIDS=()
for log in home-a home-b relay; do
    echo "----- $log"
    grep -E '^(a|b|relay):|^test |panicked|FAILED|test result' "$WORK/$log.log" || cat "$WORK/$log.log"
done
[[ $status == 0 ]] || die "FAILED"
echo "carrier-nat-test: PASS"
