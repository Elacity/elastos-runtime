#!/usr/bin/env bash
#
# ElastOS Notepad End-to-End Tests
#
# Multi-capsule architecture (runtime → sessions → capabilities → localhost-provider)
#
# Usage: cd elastos && bash tests/notepad_e2e.sh

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'

PASS=0
FAIL=0

assert_eq() {
    local desc="$1" expected="$2" actual="$3"
    if [ "$expected" = "$actual" ]; then
        echo -e "  ${GREEN}PASS${NC} $desc"
        PASS=$((PASS + 1))
    else
        echo -e "  ${RED}FAIL${NC} $desc"
        echo "    expected: $expected"
        echo "    actual:   $actual"
        FAIL=$((FAIL + 1))
    fi
}

assert_contains() {
    local desc="$1" needle="$2" haystack="$3"
    if echo "$haystack" | grep -q "$needle"; then
        echo -e "  ${GREEN}PASS${NC} $desc"
        PASS=$((PASS + 1))
    else
        echo -e "  ${RED}FAIL${NC} $desc"
        echo "    expected to contain: $needle"
        echo "    actual: $haystack"
        FAIL=$((FAIL + 1))
    fi
}

# ── Step 1: Build ──────────────────────────────────────────────────────

echo -e "${YELLOW}Step 1: Building...${NC}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$SCRIPT_DIR"

echo "  Building runtime..."
cargo build -p elastos-server --release 2>/dev/null

echo -e "  ${GREEN}Build complete${NC}"

ELASTOS="./target/release/elastos"

# ── Step 2: Multi-Capsule Architecture (real capsule binaries) ────────

echo -e "\n${YELLOW}Step 2: Multi-capsule capability flow via real capsule binaries...${NC}"

ADDR="127.0.0.1:3099"
API="http://$ADDR"
SERVER_PID=""
NOTEPAD_BIN="capsules/notepad/target/release/notepad"

cleanup_server() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    rm -f /tmp/elastos-e2e-serve.log
}
trap cleanup_server EXIT

# Build localhost-provider if needed
LP_BIN="capsules/localhost-provider/target/release/localhost-provider"
if [ ! -f "$LP_BIN" ]; then
    echo "  Building localhost-provider..."
    (cd capsules/localhost-provider && cargo build --release 2>/dev/null)
fi

# Build shell capsule if needed
SHELL_BIN="capsules/shell/target/release/shell"
if [ ! -f "$SHELL_BIN" ]; then
    echo "  Building shell capsule..."
    (cd capsules/shell && cargo build --release 2>/dev/null)
fi

# Build notepad if needed
if [ ! -f "$NOTEPAD_BIN" ]; then
    echo "  Building notepad..."
    (cd capsules/notepad && cargo build --release 2>/dev/null)
fi

# Helper: run notepad with correct env
run_notepad() {
    ELASTOS_API="$API" ELASTOS_TOKEN="$APP_TOKEN" "$NOTEPAD_BIN" "$@"
}

# 2a. Start server (runtime spawns localhost-provider + shell capsules)
echo -e "\n  ${YELLOW}2a. Start elastos serve (spawns localhost-provider + shell capsules)${NC}"
$ELASTOS serve --addr "$ADDR" > /tmp/elastos-e2e-serve.log 2>&1 &
SERVER_PID=$!

# Wait for tokens to appear in output
for i in $(seq 1 40); do
    if grep -q "App:" /tmp/elastos-e2e-serve.log 2>/dev/null; then
        break
    fi
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo -e "  ${RED}FAIL${NC} Server failed to start"
        cat /tmp/elastos-e2e-serve.log
        FAIL=$((FAIL + 1))
        break
    fi
    sleep 0.25
done

APP_TOKEN=$(grep "App:" /tmp/elastos-e2e-serve.log 2>/dev/null | awk '{print $NF}')

if [ -z "$APP_TOKEN" ]; then
    echo -e "  ${RED}FAIL${NC} Could not capture app token"
    FAIL=$((FAIL + 1))
else
    echo -e "  ${GREEN}PASS${NC} Server started, app token captured"
    PASS=$((PASS + 1))

    # Verify capsule CIDs are printed
    if grep -q "Capsule  localhost-provider" /tmp/elastos-e2e-serve.log 2>/dev/null; then
        echo -e "  ${GREEN}PASS${NC} localhost-provider CID printed"
        PASS=$((PASS + 1))
    else
        echo -e "  ${RED}FAIL${NC} localhost-provider CID not in output"
        FAIL=$((FAIL + 1))
    fi

    if grep -q "Capsule  shell" /tmp/elastos-e2e-serve.log 2>/dev/null; then
        echo -e "  ${GREEN}PASS${NC} shell CID printed"
        PASS=$((PASS + 1))
    else
        echo -e "  ${RED}FAIL${NC} shell CID not in output"
        FAIL=$((FAIL + 1))
    fi

    # Wait for health
    for i in $(seq 1 40); do
        if curl -sf "$API/api/health" >/dev/null 2>&1; then break; fi
        sleep 0.25
    done

    # 2b. Access denied without capability token
    echo -e "\n  ${YELLOW}2b. Access denied without capability${NC}"
    HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X PUT "$API/api/localhost/Users/self/Documents/Notes/test.txt" \
        -H "Authorization: Bearer $APP_TOKEN" \
        -H "Content-Type: application/octet-stream" \
        -d "test")
    assert_eq "storage write without capability returns 403" "403" "$HTTP_CODE"

    # 2c. Create note via notepad binary (request → shell grants → write)
    echo -e "\n  ${YELLOW}2c. Create note via notepad binary${NC}"
    CREATE_OUT=$(run_notepad create hello "Hello via capsule!" 2>&1)
    assert_contains "notepad create succeeds" "Created note" "$CREATE_OUT"

    # 2d. Read note via notepad binary
    echo -e "\n  ${YELLOW}2d. Read note via notepad binary${NC}"
    READ_OUT=$(run_notepad read hello 2>&1)
    assert_contains "notepad read returns content" "Hello via capsule!" "$READ_OUT"

    # 2e. List notes via notepad binary
    echo -e "\n  ${YELLOW}2e. List notes via notepad binary${NC}"
    LIST_OUT=$(run_notepad list 2>&1)
    assert_contains "notepad list shows hello" "hello" "$LIST_OUT"

    # 2f. Edit note via notepad binary
    echo -e "\n  ${YELLOW}2f. Edit note via notepad binary${NC}"
    EDIT_OUT=$(run_notepad edit hello "Updated content" 2>&1)
    assert_contains "notepad edit succeeds" "Updated note" "$EDIT_OUT"

    # 2g. Verify edit via notepad binary
    echo -e "\n  ${YELLOW}2g. Verify edit${NC}"
    READ2_OUT=$(run_notepad read hello 2>&1)
    assert_contains "notepad read returns updated content" "Updated content" "$READ2_OUT"

    # 2h. Delete note via notepad binary
    echo -e "\n  ${YELLOW}2h. Delete note via notepad binary${NC}"
    DEL_OUT=$(run_notepad delete hello 2>&1)
    assert_contains "notepad delete succeeds" "Deleted note" "$DEL_OUT"
fi

cleanup_server

# ── Results ────────────────────────────────────────────────────────────

echo -e "\n════════════════════════════════════════"
TOTAL=$((PASS + FAIL))
if [ "$FAIL" -eq 0 ]; then
    echo -e "${GREEN}ALL $TOTAL TESTS PASSED${NC}"
else
    echo -e "${RED}$FAIL/$TOTAL TESTS FAILED${NC}"
fi
echo "════════════════════════════════════════"

exit "$FAIL"
