#!/usr/bin/env bash
#
# ElastOS AI Agent
#
# Capsules used (P2P via built-in Carrier):
#   localhost-provider — encrypted local storage
#   shell             — auto-grants capabilities
#   did-provider      — DID identity (did:key, Ed25519)
#   llama-provider    — local llama-server subprocess management
#   ai-provider       — LLM backend routing (local llama, Venice)
#   agent             — headless AI chat agent
#
# Usage:
#   ./scripts/agent.sh                                  # Default (nick=agent, backend=local)
#   ./scripts/agent.sh --nick claude --backend local    # Named agent with local llama
#   ./scripts/agent.sh --nick claude --respond-all      # Respond to all messages
#   ./scripts/agent.sh --connect "<ticket>"             # Connect to peer via ticket
#   ./scripts/agent.sh --help                           # Show help
#

set -euo pipefail

BOLD='\033[1m'
DIM='\033[2m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
NC='\033[0m'

# Navigate to project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

source "$(dirname "${BASH_SOURCE[0]}")/resolve-binary.sh"
ELASTOS="$REPO_ELASTOS_BIN"
runtime_version() { "$ELASTOS" --version 2>/dev/null | head -1; }

# ── Help ──────────────────────────────────────────────────────────────

show_help() {
    echo ""
    echo -e "${BOLD}ElastOS AI Agent${NC}"
    echo ""
    echo "  Launches an AI agent that joins P2P chat and responds via LLM."
    echo "  Requires an LLM backend (default: local llama-server)."
    echo ""
    echo -e "${BOLD}Usage:${NC}"
    echo "  ./scripts/agent.sh                              Default agent"
    echo "  ./scripts/agent.sh --nick claude                Named agent"
    echo "  ./scripts/agent.sh --backend local              Use local llama-server"
    echo "  ./scripts/agent.sh --respond-all                Respond to all messages"
    echo ""
    echo -e "${BOLD}Options:${NC}"
    echo "  --nick <name>        Agent nickname (default: agent)"
    echo "  --channel <name>     Channel to join (default: #general)"
    echo "  --backend <name>     AI backend: local, venice (default: local)"
    echo "  --respond-all        Respond to all messages, not just @mentions"
    echo "  --connect <ticket>   Connect to peer via ticket (cross-device bootstrap)"
    echo "  --rebuild            Force rebuild all capsules"
    echo "  --help               Show this help"
    echo ""
    echo -e "${BOLD}Prerequisites:${NC}"
    echo "  For --backend local:  elastos setup --with llama-server,model-qwen3.5-0.8b"
    echo "  For --backend venice: export VENICE_API_KEY=sk-..."
    echo ""
    exit 0
}

# ── Parse args ────────────────────────────────────────────────────────

FORCE_REBUILD=false
PASSTHROUGH_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h)
            show_help
            ;;
        --rebuild)
            FORCE_REBUILD=true
            shift
            ;;
        *)
            PASSTHROUGH_ARGS+=("$1")
            shift
            ;;
    esac
done

# ── Build ─────────────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}ElastOS AI Agent${NC}"
echo -e "${DIM}Capsules: storage + shell + identity + llama + AI + agent${NC}"
echo ""

needs_build() {
    local bin="$1"
    local src_dir="$2"
    if [ "$FORCE_REBUILD" = true ]; then return 0; fi
    if [ ! -f "$bin" ]; then return 0; fi
    if [ "$(find "$src_dir" -name '*.rs' -newer "$bin" 2>/dev/null | head -1)" ]; then return 0; fi
    return 1
}

show() { echo -e "  ${CYAN}▶${NC} $*"; }

BUILDS_NEEDED=false

# Runtime
if needs_build "$ELASTOS" "elastos/crates/"; then
    show "Building runtime..."
    BUILDS_NEEDED=true
    (cd elastos && cargo build -p elastos-server --release 2>/dev/null)
fi

# localhost-provider (core)
LP_BIN="elastos/capsules/localhost-provider/target/release/localhost-provider"
if needs_build "$LP_BIN" "elastos/capsules/localhost-provider/src/"; then
    show "Building localhost-provider..."
    BUILDS_NEEDED=true
    (cd elastos/capsules/localhost-provider && cargo build --release 2>/dev/null)
fi

# shell (core)
SHELL_BIN="elastos/capsules/shell/target/release/shell"
if needs_build "$SHELL_BIN" "elastos/capsules/shell/src/"; then
    show "Building shell..."
    BUILDS_NEEDED=true
    (cd elastos/capsules/shell && cargo build --release 2>/dev/null)
fi

# did-provider
DID_BIN="capsules/did-provider/target/release/did-provider"
if needs_build "$DID_BIN" "capsules/did-provider/src/"; then
    show "Building did-provider..."
    BUILDS_NEEDED=true
    (cd capsules/did-provider && cargo build --release 2>/dev/null)
fi

# ai-provider
AI_BIN="capsules/ai-provider/target/release/ai-provider"
if needs_build "$AI_BIN" "capsules/ai-provider/src/"; then
    show "Building ai-provider..."
    BUILDS_NEEDED=true
    (cd capsules/ai-provider && cargo build --release 2>/dev/null)
fi

# llama-provider
LLAMA_BIN="capsules/llama-provider/target/release/llama-provider"
if needs_build "$LLAMA_BIN" "capsules/llama-provider/src/"; then
    show "Building llama-provider..."
    BUILDS_NEEDED=true
    (cd capsules/llama-provider && cargo build --release 2>/dev/null)
fi

# agent
AGENT_BIN="capsules/agent/target/release/agent"
if needs_build "$AGENT_BIN" "capsules/agent/src/"; then
    show "Building agent..."
    BUILDS_NEEDED=true
    (cd capsules/agent && cargo build --release 2>/dev/null)
fi

if [ "$BUILDS_NEEDED" = true ]; then
    echo -e "  ${GREEN}Build complete.${NC}"
else
    echo -e "  ${DIM}All binaries up to date.${NC}"
fi
echo ""

[[ -x "$ELASTOS" ]] || { echo -e "${RED}Error:${NC} repo elastos binary not found at ${REPO_ELASTOS_BIN}" >&2; exit 1; }
echo -e "${DIM}Runtime: ${ELASTOS} (${YELLOW}$(runtime_version)${DIM})${NC}"
echo ""

# ── Launch ────────────────────────────────────────────────────────────

echo -e "${GREEN}Launching agent...${NC}"
echo -e "${DIM}Agent will respond to @mentions in the gossip channel${NC}"
echo ""

exec "$ELASTOS" agent "${PASSTHROUGH_ARGS[@]}"
