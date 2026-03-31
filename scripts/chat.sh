#!/usr/bin/env bash
#
# Host chat launcher.
# Reuses installed runtime state by default.
# Native `elastos chat` is rootless and should not need sudo or CAP_NET_ADMIN.

set -euo pipefail

BOLD='\033[1m'
DIM='\033[2m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
NC='\033[0m'

die()  { echo -e "${RED}Error:${NC} $*" >&2; exit 1; }
info() { echo -e "  ${GREEN}▶${NC} $*"; }
warn() { echo -e "  ${YELLOW}!${NC} $*"; }
runtime_version() { "$ELASTOS_BIN" --version 2>/dev/null | head -1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

source "$SCRIPT_DIR/resolve-binary.sh"
HOST_DATA_DIR="${HOME}/.local/share/elastos"

NICK=""
CONNECT=""
REBUILD=false
FORCE_INSTALLED=false
PASSTHROUGH_ARGS=()
ELASTOS_BIN="$REPO_ELASTOS_BIN"

show_help() {
    echo ""
    echo -e "${BOLD}ElastOS Chat${NC}"
    echo ""
    echo "Usage:"
    echo "  ./scripts/chat.sh --nick alice"
    echo "  ./scripts/chat.sh --nick bob --connect \"<ticket>\""
    echo ""
    echo "Options:"
    echo "  --nick <name>        Chat nickname"
    echo "  --connect <ticket>   Peer ticket"
    echo "  --rebuild            Rebuild repo runtime before launch"
    echo "  --installed          Force use of installed ~/.local/bin/elastos"
    echo "  --sudo               Deprecated no-op (native chat should not need sudo)"
    echo "  --help               Show this help"
    echo ""
    echo "Expected prerequisites:"
    echo "  1. elastos binary built or installed"
    echo "  2. elastos serve running in another terminal"
    echo ""
}

require_arg_value() {
    local flag="$1"
    local value="${2:-}"
    if [[ -z "$value" || "$value" == --* ]]; then
        die "${flag} requires a value"
    fi
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h)
            show_help
            exit 0
            ;;
        --nick)
            require_arg_value "$1" "${2:-}"
            NICK="$2"
            PASSTHROUGH_ARGS+=("$1" "$2")
            shift 2
            ;;
        --connect)
            require_arg_value "$1" "${2:-}"
            CONNECT="$2"
            PASSTHROUGH_ARGS+=("$1" "$2")
            shift 2
            ;;
        --rebuild)
            REBUILD=true
            shift
            ;;
        --installed)
            FORCE_INSTALLED=true
            shift
            ;;
        --sudo)
            warn "--sudo is deprecated for native chat and will be ignored"
            shift
            ;;
        *)
            PASSTHROUGH_ARGS+=("$1")
            shift
            ;;
    esac
done

if [[ "$FORCE_INSTALLED" == true ]]; then
    [[ -x "$INSTALLED_ELASTOS_BIN" ]] || die "installed elastos not found at ${INSTALLED_ELASTOS_BIN}"
    ELASTOS_BIN="$INSTALLED_ELASTOS_BIN"
fi

echo ""
echo -e "${BOLD}ElastOS Chat${NC}"
if [[ "$ELASTOS_BIN" == "$REPO_ELASTOS_BIN" ]]; then
    echo -e "${DIM}dev checkout path (repo runtime preferred)${NC}"
else
    echo -e "${DIM}installed-runtime path${NC}"
fi
echo ""

if [[ "$ELASTOS_BIN" == "$REPO_ELASTOS_BIN" ]]; then
    [[ -x "$ELASTOS_BIN" ]] || die "repo elastos binary not found at ${REPO_ELASTOS_BIN} (build it or use --installed)"
else
    [[ -x "$ELASTOS_BIN" ]] || die "installed elastos binary not found at ${INSTALLED_ELASTOS_BIN}"
fi

if [[ -z "$NICK" ]]; then
    echo -ne "${CYAN}?${NC} ${BOLD}Nickname${NC} ${DIM}(default: anon)${NC}: "
    read -r input_nick
    if [[ -n "$input_nick" ]]; then
        NICK="$input_nick"
    else
        NICK="anon"
    fi
    PASSTHROUGH_ARGS+=("--nick" "$NICK")
fi

if [[ -z "$CONNECT" ]]; then
    echo -ne "${CYAN}?${NC} ${BOLD}Connect ticket${NC} ${DIM}(optional, Enter to skip)${NC}: "
    read -r input_ticket
    if [[ -n "$input_ticket" ]]; then
        CONNECT="$input_ticket"
        PASSTHROUGH_ARGS+=("--connect" "$CONNECT")
    fi
fi

if [[ "$REBUILD" == true ]]; then
    [[ -x "$REPO_ELASTOS_BIN" ]] || die "--rebuild requires repo runtime at ${REPO_ELASTOS_BIN}"
    ELASTOS_BIN="$REPO_ELASTOS_BIN"
    info "Building runtime (elastos-server)..."
    (cd elastos && cargo build --release -p elastos-server)
fi

# Native chat only needs the runtime running — no crosvm/vmlinux/kubo.

info "Runtime: ${ELASTOS_BIN}"
info "Version: $(runtime_version)"
info "Nick: ${NICK}"
echo ""

exec "$ELASTOS_BIN" chat "${PASSTHROUGH_ARGS[@]}"
