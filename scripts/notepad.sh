#!/usr/bin/env bash
#
# ElastOS Multi-Capsule Demo
#
# Three real capsule binaries, each content-addressed:
#   1. localhost-provider — storage capsule (spawned by runtime)
#   2. shell          — auto-grants capabilities (spawned by runtime)
#   3. notepad    — CLI notepad (run per command by this script)
#
# notepad.sh is just a UI — it does NOT simulate any capsule behavior.
#
# Usage:
#   ./scripts/notepad.sh          # Interactive REPL
#   ./scripts/notepad.sh --auto   # Scripted walkthrough
#

set -euo pipefail

BOLD='\033[1m'
DIM='\033[2m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
NC='\033[0m'

# ── Help ──────────────────────────────────────────────────────────────

show_help() {
    echo ""
    echo -e "${BOLD}ElastOS Notepad Demo${NC}"
    echo ""
    echo "  Demonstrates the capability token system with three real capsules:"
    echo "  runtime (API + token signing), shell (auto-grant), and localhost-provider"
    echo "  (encrypted storage). The notepad binary requests capabilities at runtime —"
    echo "  without a valid Ed25519-signed token, every operation gets HTTP 403."
    echo ""
    echo -e "${BOLD}Usage:${NC}"
    echo "  ./scripts/notepad.sh           Interactive REPL"
    echo "  ./scripts/notepad.sh --auto    Scripted walkthrough (watch the demo)"
    echo "  ./scripts/notepad.sh --help    Show this help"
    echo ""
    echo -e "${BOLD}Commands (in interactive mode):${NC}"
    echo "  create <name> <content>   Create a note"
    echo "  read <name>               Read a note"
    echo "  edit <name> <content>     Update a note"
    echo "  delete <name>             Delete a note"
    echo "  list                      List all notes"
    echo "  denied                    Demo: show access denied without capability"
    echo "  help                      Show commands"
    echo "  quit                      Exit"
    echo ""
    exit 0
}

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    show_help
fi

# Navigate to project root (scripts/ is one level down)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

source "$(dirname "${BASH_SOURCE[0]}")/resolve-binary.sh"
ELASTOS="$REPO_ELASTOS_BIN"
NOTEPAD="capsules/notepad/target/release/notepad"
ADDR="127.0.0.1:3000"
API="http://$ADDR"
SERVER_PID=""
APP_TOKEN=""

# ── Helpers ──────────────────────────────────────────────────────────

show()  { echo -e "\n${CYAN}▶${NC} ${BOLD}$*${NC}"; }
ok()    { echo -e "  ${GREEN}✓${NC} $1"; }
err()   { echo -e "  ${RED}✗${NC} $1"; }
runtime_version() { "$ELASTOS" --version 2>/dev/null | head -1; }

cleanup() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    rm -f /tmp/elastos-demo-serve.log
}
trap cleanup EXIT INT TERM

# ── Build ────────────────────────────────────────────────────────────

echo -e "${BOLD}ElastOS Multi-Capsule Demo${NC}"
echo -e "${DIM}Three real capsules: localhost-provider + shell + notepad${NC}"
echo ""

# Build runtime if needed
if [ ! -x "$ELASTOS" ] || [ "$(find elastos/crates/ -name '*.rs' -newer "$ELASTOS" 2>/dev/null | head -1)" ]; then
    show "Building runtime..."
    (cd elastos && cargo build -p elastos-server --release 2>/dev/null)
fi

[[ -x "$ELASTOS" ]] || { echo -e "${RED}Error:${NC} repo elastos binary not found at ${REPO_ELASTOS_BIN}" >&2; exit 1; }

# Build localhost-provider if needed (core — in elastos/capsules/)
LP_BIN="elastos/capsules/localhost-provider/target/release/localhost-provider"
if [ ! -f "$LP_BIN" ] || [ "$(find elastos/capsules/localhost-provider/src/ -name '*.rs' -newer "$LP_BIN" 2>/dev/null | head -1)" ]; then
    show "Building localhost-provider capsule..."
    (cd elastos/capsules/localhost-provider && cargo build --release 2>/dev/null)
fi

# Build shell if needed (core — in elastos/capsules/)
SHELL_BIN="elastos/capsules/shell/target/release/shell"
if [ ! -f "$SHELL_BIN" ] || [ "$(find elastos/capsules/shell/src/ -name '*.rs' -newer "$SHELL_BIN" 2>/dev/null | head -1)" ]; then
    show "Building shell capsule..."
    (cd elastos/capsules/shell && cargo build --release 2>/dev/null)
fi

# Build notepad if needed
if [ ! -f "$NOTEPAD" ] || [ "$(find capsules/notepad/src/ -name '*.rs' -newer "$NOTEPAD" 2>/dev/null | head -1)" ]; then
    show "Building notepad capsule..."
    (cd capsules/notepad && cargo build --release 2>/dev/null)
fi

echo -e "${GREEN}Build complete.${NC}"
echo -e "${DIM}Runtime: ${ELASTOS} (${YELLOW}$(runtime_version)${DIM})${NC}"

# ── Start Server ─────────────────────────────────────────────────────

show "Starting ElastOS runtime..."

# Kill a leftover ElastOS runtime on this address (don't kill unrelated processes)
OLD_PIDS=$(pgrep -f "elastos serve --addr $ADDR" || true)
if [ -n "$OLD_PIDS" ]; then
    echo -e "  ${DIM}Killing leftover ElastOS runtime PID(s): $OLD_PIDS${NC}"
    kill $OLD_PIDS 2>/dev/null || true
    sleep 1
fi

$ELASTOS serve --addr "$ADDR" > /tmp/elastos-demo-serve.log 2>&1 &
SERVER_PID=$!

# Wait for server to be ready (parse tokens from output)
for i in $(seq 1 30); do
    if grep -q "App:" /tmp/elastos-demo-serve.log 2>/dev/null; then
        break
    fi
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo -e "${RED}Server failed to start:${NC}"
        cat /tmp/elastos-demo-serve.log
        exit 1
    fi
    sleep 0.2
done

APP_TOKEN=$(grep "App:" /tmp/elastos-demo-serve.log | awk '{print $NF}')

if [ -z "$APP_TOKEN" ]; then
    echo -e "${RED}Failed to capture app token${NC}"
    cat /tmp/elastos-demo-serve.log
    exit 1
fi

# Wait for health endpoint
HEALTH_OK=false
for i in $(seq 1 30); do
    if curl -sf "$API/api/health" >/dev/null 2>&1; then
        HEALTH_OK=true
        break
    fi
    sleep 0.2
done
if [ "$HEALTH_OK" = false ]; then
    echo -e "${RED}Runtime health check failed (${API}/api/health)${NC}"
    cat /tmp/elastos-demo-serve.log
    exit 1
fi

# Print capsule CIDs from runtime output
echo ""
if grep -q "Capsule" /tmp/elastos-demo-serve.log 2>/dev/null; then
    grep "Capsule" /tmp/elastos-demo-serve.log | while read -r line; do
        echo -e "  ${BOLD}${line}${NC}"
    done
fi
echo -e "  ${BOLD}Runtime${NC}   API             ${GREEN}$API${NC}"

# ── Notepad helper ───────────────────────────────────────────────────

# Run notepad binary with the correct environment
notepad() {
    ELASTOS_API="$API" ELASTOS_TOKEN="$APP_TOKEN" "$NOTEPAD" "$@"
}

# ── Demo: Permission Denied Without Capability ───────────────────────

demo_denied() {
    show "Without capability token: Permission Denied"
    echo -e "  ${DIM}\$ curl PUT /api/localhost/Users/self/Documents/Notes/test.txt (no X-Capability-Token)${NC}"
    local http_code
    http_code=$(curl -s -o /dev/null -w "%{http_code}" -X PUT "$API/api/localhost/Users/self/Documents/Notes/test.txt" \
        -H "Authorization: Bearer $APP_TOKEN" \
        -H "Content-Type: application/octet-stream" \
        -d "test" || true)
    if [ -z "$http_code" ]; then
        err "Runtime API not reachable at $API"
        return 1
    fi
    if [ "$http_code" = "403" ]; then
        ok "HTTP 403 Forbidden — zero ambient authority enforced"
    else
        err "Expected 403, got $http_code"
    fi
}

# ── Demo Commands (all via real notepad binary) ──────────────────

cmd_create() {
    local name="$1"; shift
    local content="$*"
    show "notepad create $name \"$content\""
    notepad create "$name" "$content"
}

cmd_read() {
    local name="$1"
    show "notepad read $name"
    notepad read "$name"
}

cmd_list() {
    show "notepad list"
    notepad list
}

cmd_edit() {
    local name="$1"; shift
    local content="$*"
    show "notepad edit $name \"$content\""
    notepad edit "$name" "$content"
}

cmd_delete() {
    local name="$1"
    show "notepad delete $name"
    notepad delete "$name"
}

cmd_help() {
    echo ""
    echo "  Commands:"
    echo "    create <name> <content>  — Create a note (via notepad binary)"
    echo "    read <name>              — Read a note"
    echo "    edit <name> <content>    — Edit a note"
    echo "    delete <name>            — Delete a note"
    echo "    list                     — List all notes"
    echo "    denied                   — Show access denied without capability"
    echo "    help                     — Show this help"
    echo "    quit                     — Exit"
    echo ""
    echo "  Every command runs the notepad binary which:"
    echo "    [1] Requests capability from runtime"
    echo "    [2] Shell capsule auto-grants Ed25519 signed token"
    echo "    [3] Uses token to access storage via localhost-provider capsule"
}

# ── Auto Mode ────────────────────────────────────────────────────────

run_auto() {
    echo ""
    echo "════════════════════════════════════════════════════════════════"
    echo -e "${BOLD}Scripted Demo${NC}"
    echo "════════════════════════════════════════════════════════════════"

    demo_denied

    cmd_create hello "Hello from ElastOS!"
    cmd_create shopping "Milk, eggs, bread"
    cmd_read hello
    cmd_list
    cmd_edit hello "Updated via capability token"
    cmd_read hello
    cmd_delete shopping
    cmd_list

    echo ""
    echo "════════════════════════════════════════════════════════════════"
    echo -e "${GREEN}${BOLD}Demo complete.${NC}"
    echo ""
    echo "What just happened:"
    echo "  1. Runtime started + auto-spawned localhost-provider & shell capsules"
    echo "  2. App session had ZERO ambient authority (403 without token)"
    echo "  3. notepad binary: request → shell grants → Ed25519 signed token"
    echo "  4. Storage routed: API → ProviderBridge → localhost-provider → disk"
    echo "  5. Three real capsule binaries, each content-addressed with SHA-256"
    echo ""
}

# ── Interactive Mode ─────────────────────────────────────────────────

run_interactive() {
    echo ""
    echo "════════════════════════════════════════════════════════════════"
    echo -e "${BOLD}Interactive Notepad${NC}"
    echo "════════════════════════════════════════════════════════════════"

    demo_denied
    cmd_help

    while true; do
        echo -ne "\n${CYAN}notepad>${NC} "
        if ! read -r line; then
            break
        fi

        # Skip empty lines
        [ -z "$line" ] && continue

        # Parse command and arguments
        local cmd arg1 rest
        cmd=$(echo "$line" | awk '{print $1}')
        arg1=$(echo "$line" | awk '{print $2}' | sed "s/^[\"']//;s/[\"']$//")
        rest=$(echo "$line" | sed 's/^[^ ]* [^ ]* //' | sed "s/^[\"']//;s/[\"']$//")

        case "$cmd" in
            create)
                if [ -z "$arg1" ] || [ "$arg1" = "$rest" -a -z "$rest" ]; then
                    echo "  Usage: create <name> <content>"
                else
                    [ "$arg1" = "$rest" ] && rest=""
                    if [ -z "$rest" ]; then
                        echo "  Usage: create <name> <content>"
                    else
                        cmd_create "$arg1" "$rest"
                    fi
                fi
                ;;
            read)
                if [ -z "$arg1" ]; then
                    echo "  Usage: read <name>"
                else
                    cmd_read "$arg1"
                fi
                ;;
            edit)
                if [ -z "$arg1" ] || [ -z "$rest" ] || [ "$arg1" = "$rest" ]; then
                    echo "  Usage: edit <name> <content>"
                else
                    cmd_edit "$arg1" "$rest"
                fi
                ;;
            delete)
                if [ -z "$arg1" ]; then
                    echo "  Usage: delete <name>"
                else
                    cmd_delete "$arg1"
                fi
                ;;
            list)
                cmd_list
                ;;
            denied)
                demo_denied
                ;;
            help)
                cmd_help
                ;;
            quit|exit)
                break
                ;;
            *)
                echo "  Unknown command: $cmd (type 'help')"
                ;;
        esac
    done

    echo ""
    echo -e "${GREEN}Goodbye.${NC}"
}

# ── Main ─────────────────────────────────────────────────────────────

if [ "${1:-}" = "--auto" ]; then
    run_auto
else
    run_interactive
fi
