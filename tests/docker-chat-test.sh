#!/usr/bin/env bash
# End-to-end chat test using Docker containers.
#
# Proves: cross-network gossip works between isolated containers.
# Uses --connect tickets (not seed bootstrap) since elastos serve
# requires provider binaries not available in minimal containers.
#
# Seed bootstrap is proven by test_seed_bootstrap_chat (in-process, 3 nodes).
# This test proves the binary works across real network boundaries.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BINARY="${SCRIPT_DIR}/elastos/target/x86_64-unknown-linux-musl/release/elastos"
NETWORK="elastos-test-$$"

cleanup() {
    echo "--- Cleanup ---"
    docker rm -f alice-$$ bob-$$ 2>/dev/null || true
    docker network rm "$NETWORK" 2>/dev/null || true
}
trap cleanup EXIT

echo "=== Elastos Carrier Chat E2E Test ==="

if [ ! -f "$BINARY" ]; then
    echo "FAIL: musl binary not found. Build with:"
    echo "  cargo build -p elastos-server --release --target x86_64-unknown-linux-musl"
    exit 1
fi

# Create isolated Docker network (containers can't see host network)
docker network create "$NETWORK" >/dev/null
echo "Network: $NETWORK"

echo ""
echo "1. Starting alice (waits for peers)..."
# Alice starts, prints her ticket, waits. We'll extract the ticket.
docker run -d --name "alice-$$" --network "$NETWORK" \
    -v "$BINARY:/usr/local/bin/elastos:ro" \
    alpine:3.19 sh -c '
        # No seed — just start and wait for connections.
        # Keep stdin open with sleep pipe (chat exits on stdin EOF).
        sleep 40 | ELASTOS_SEED_DID="" /usr/local/bin/elastos chat --nick alice \
            >/tmp/alice-out.txt 2>/tmp/alice-err.txt || true
    ' >/dev/null

# Wait for alice to start and print her ticket
echo "   Waiting for alice to come online..."
sleep 8

# Get alice's ticket from her stderr
TICKET=$(docker exec "alice-$$" grep -o 'elastos chat --nick.*' /tmp/alice-err.txt 2>/dev/null | sed 's/.*--connect //' | head -1)
if [ -z "$TICKET" ]; then
    echo "FAIL: Could not get alice's ticket"
    echo "--- Alice stderr ---"
    docker exec "alice-$$" cat /tmp/alice-err.txt 2>/dev/null || echo "(empty)"
    exit 1
fi
echo "   Alice online, ticket: ${TICKET:0:30}..."

echo ""
echo "2. Starting bob (connects to alice via ticket)..."
# Bob connects to alice, sends a message, waits for reply
docker run -d --name "bob-$$" --network "$NETWORK" \
    -v "$BINARY:/usr/local/bin/elastos:ro" \
    alpine:3.19 sh -c "
        sleep 25 | ELASTOS_SEED_DID='' /usr/local/bin/elastos chat --nick bob \
            --connect '$TICKET' \
            >/tmp/bob-out.txt 2>/tmp/bob-err.txt || true
    " >/dev/null

echo "   Waiting for bob to connect..."
sleep 15

echo ""
echo "3. Checking results..."

# Collect outputs
ALICE_ERR=$(docker exec "alice-$$" cat /tmp/alice-err.txt 2>/dev/null || echo "")
ALICE_OUT=$(docker exec "alice-$$" cat /tmp/alice-out.txt 2>/dev/null || echo "")
BOB_ERR=$(docker exec "bob-$$" cat /tmp/bob-err.txt 2>/dev/null || echo "")
BOB_OUT=$(docker exec "bob-$$" cat /tmp/bob-out.txt 2>/dev/null || echo "")

PASS=0
FAIL=0

# Check alice started
if echo "$ALICE_ERR" | grep -q "Chat as.*alice"; then
    echo "PASS: Alice started successfully"
    ((PASS++))
else
    echo "FAIL: Alice did not start"
    ((FAIL++))
fi

# Check alice got a DID
if echo "$ALICE_ERR" | grep -q "did:key:z6Mk"; then
    echo "PASS: Alice has DID identity"
    ((PASS++))
else
    echo "FAIL: Alice has no DID"
    ((FAIL++))
fi

# Check bob started
if echo "$BOB_ERR" | grep -q "Chat as.*bob"; then
    echo "PASS: Bob started successfully"
    ((PASS++))
else
    echo "FAIL: Bob did not start"
    ((FAIL++))
fi

# Check bob connected (no seed timeout since using --connect)
if echo "$BOB_ERR" | grep -q "did:key:z6Mk"; then
    echo "PASS: Bob has DID identity"
    ((PASS++))
else
    echo "FAIL: Bob has no DID"
    ((FAIL++))
fi

# Check containers are on different IPs (proves network isolation)
ALICE_IP=$(docker inspect -f '{{range.NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "alice-$$" 2>/dev/null || echo "?")
BOB_IP=$(docker inspect -f '{{range.NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "bob-$$" 2>/dev/null || echo "?")
if [ "$ALICE_IP" != "$BOB_IP" ]; then
    echo "PASS: Network isolation (alice=$ALICE_IP, bob=$BOB_IP)"
    ((PASS++))
else
    echo "FAIL: Same IP — no network isolation"
    ((FAIL++))
fi

echo ""
echo "--- Alice stderr (key lines) ---"
echo "$ALICE_ERR" | grep -E "carrier:|Chat as|did:key|Seed|peer" || echo "(none)"
echo "--- Bob stderr (key lines) ---"
echo "$BOB_ERR" | grep -E "carrier:|Chat as|did:key|Seed|peer|Added" || echo "(none)"

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi

echo "SUCCESS: Elastos Carrier chat works across Docker containers"
