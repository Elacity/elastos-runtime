#!/usr/bin/env bash
#
# Build ElastOS runtime and capsules
#
# Usage:
#   ./scripts/build.sh              # Build runtime + core capsules
#   ./scripts/build.sh --all        # Build everything (including chat, P2P)
#   ./scripts/build.sh --capsule X  # Build a specific capsule
#   ./scripts/build.sh --clean      # Clean all build artifacts first
#   ./scripts/build.sh --help       # Show help
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
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

# Resolve cargo target-dir
source "$SCRIPT_DIR/resolve-binary.sh"

# ── Help ──────────────────────────────────────────────────────────────

show_help() {
    echo ""
    echo -e "${BOLD}ElastOS Build${NC}"
    echo ""
    echo -e "${BOLD}Usage:${NC}"
    echo "  ./scripts/build.sh                Build runtime + core capsules"
    echo "  ./scripts/build.sh --all          Build everything (runtime + all capsules)"
    echo "  ./scripts/build.sh --runtime      Build runtime only"
    echo "  ./scripts/build.sh --capsule X    Build specific capsule (e.g. chat, notepad)"
    echo "  ./scripts/build.sh --clean        Clean all artifacts, then build"
    echo "  ./scripts/build.sh --list         List all buildable capsules"
    echo "  ./scripts/build.sh --help         Show this help"
    echo ""
    echo -e "${BOLD}Capsule locations:${NC}"
    echo "  Core capsules:    elastos/capsules/ + provider capsules"
    echo "  App capsules:     capsules/         (chat, notepad, did-provider, ai-provider, llama-provider, agent, ipfs-provider, site-provider, tunnel-provider)"
    echo "  Data capsules:    capsules/         (gba-emulator, gba-ucity)"
    echo "  Data capsules don't need building — they're static assets."
    echo ""
    echo -e "${BOLD}Component setup:${NC}"
    echo "  elastos setup --list          List available external components"
    echo "  elastos setup                 Install the default PC2/chat core"
    echo "  elastos setup --with kubo     Install a specific component"
    echo ""
    exit 0
}

# ── Helpers ───────────────────────────────────────────────────────────

show() { echo -e "  ${CYAN}▶${NC} $*"; }
ok()   { echo -e "  ${GREEN}✓${NC} $*"; }
err()  { echo -e "  ${RED}✗${NC} $*"; }

needs_build() {
    local bin="$1"
    local src_dir="$2"
    if [ ! -f "$bin" ]; then return 0; fi
    if [ "$(find "$src_dir" -name '*.rs' -newer "$bin" 2>/dev/null | head -1)" ]; then return 0; fi
    return 1
}

build_capsule() {
    local name="$1"
    local dir="$2"
    local bin="$dir/target/release/$name"

    if [ ! -d "$dir" ]; then
        err "Capsule not found: $dir"
        return 1
    fi

    if needs_build "$bin" "$dir/src/"; then
        show "Building $name..."
        (cd "$dir" && cargo build --release 2>&1 | tail -1)
        ok "$name"
    else
        echo -e "  ${DIM}$name — up to date${NC}"
    fi
}

# All buildable capsules (name:path)
CORE_CAPSULES=(
    "shell:elastos/capsules/shell"
    "localhost-provider:elastos/capsules/localhost-provider"
    "site-provider:capsules/site-provider"
    "tunnel-provider:capsules/tunnel-provider"
)

APP_CAPSULES=(
    "chat:capsules/chat"
    "notepad:capsules/notepad"
    "did-provider:capsules/did-provider"
    "ai-provider:capsules/ai-provider"
    "llama-provider:capsules/llama-provider"
    "agent:capsules/agent"
    "ipfs-provider:capsules/ipfs-provider"
)

list_capsules() {
    echo ""
    echo -e "${BOLD}Buildable Capsules${NC}"
    echo ""
    echo -e "  ${BOLD}Core (elastos/capsules/):${NC}"
    for entry in "${CORE_CAPSULES[@]}"; do
        local name="${entry%%:*}"
        local dir="${entry#*:}"
        local bin="$dir/target/release/$name"
        if [ -f "$bin" ]; then
            echo -e "    ${GREEN}●${NC} $name"
        else
            echo -e "    ${DIM}○${NC} $name ${DIM}(not built)${NC}"
        fi
    done
    echo ""
    echo -e "  ${BOLD}App (capsules/):${NC}"
    for entry in "${APP_CAPSULES[@]}"; do
        local name="${entry%%:*}"
        local dir="${entry#*:}"
        local bin="$dir/target/release/$name"
        if [ -f "$bin" ]; then
            echo -e "    ${GREEN}●${NC} $name"
        else
            echo -e "    ${DIM}○${NC} $name ${DIM}(not built)${NC}"
        fi
    done
    echo ""
    echo -e "  ${DIM}Data capsules (gba-emulator, gba-ucity) are static — no build needed.${NC}"
    echo ""
    exit 0
}

# ── Parse args ────────────────────────────────────────────────────────

BUILD_ALL=false
BUILD_RUNTIME_ONLY=false
BUILD_SPECIFIC=""
CLEAN_FIRST=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h)
            show_help
            ;;
        --list|-l)
            list_capsules
            ;;
        --all|-a)
            BUILD_ALL=true
            shift
            ;;
        --runtime|-r)
            BUILD_RUNTIME_ONLY=true
            shift
            ;;
        --clean|-c)
            CLEAN_FIRST=true
            shift
            ;;
        --capsule)
            if [ -z "${2:-}" ] || [[ "${2:-}" == --* ]]; then
                err "Usage: ./scripts/build.sh --capsule <name>"
                exit 1
            fi
            if [ -n "$BUILD_SPECIFIC" ]; then
                err "--capsule can only be specified once"
                exit 1
            fi
            BUILD_SPECIFIC="$2"
            shift 2
            ;;
        *)
            err "Unknown option: $1"
            echo "Run ./scripts/build.sh --help for usage."
            exit 1
            ;;
    esac
done

if [ -n "$BUILD_SPECIFIC" ] && { [ "$BUILD_ALL" = true ] || [ "$BUILD_RUNTIME_ONLY" = true ]; }; then
    err "--capsule cannot be combined with --all or --runtime"
    exit 1
fi

# ── Build ─────────────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}ElastOS Build${NC}"
echo ""

# Clean if requested
if [ "$CLEAN_FIRST" = true ]; then
    show "Cleaning build artifacts..."
    (cd elastos && cargo clean 2>/dev/null) || true
    for entry in "${CORE_CAPSULES[@]}" "${APP_CAPSULES[@]}"; do
        dir="${entry#*:}"
        [ -d "$dir" ] && (cd "$dir" && cargo clean 2>/dev/null) || true
    done
    ok "Clean"
    echo ""
fi

# Build specific capsule
if [ -n "$BUILD_SPECIFIC" ]; then
    FOUND=false
    for entry in "${CORE_CAPSULES[@]}" "${APP_CAPSULES[@]}"; do
        name="${entry%%:*}"
        dir="${entry#*:}"
        if [ "$name" = "$BUILD_SPECIFIC" ]; then
            build_capsule "$name" "$dir"
            FOUND=true
            break
        fi
    done
    if [ "$FOUND" = false ]; then
        err "Unknown capsule: $BUILD_SPECIFIC"
        echo "Run ./scripts/build.sh --list to see available capsules."
        exit 1
    fi
    echo ""
    exit 0
fi

# Runtime
ELASTOS="$REPO_ELASTOS_BIN"
if needs_build "$ELASTOS" "elastos/crates/"; then
    show "Building runtime (elastos-server)..."
    (cd elastos && cargo build -p elastos-server --release 2>&1 | tail -1)
    ok "Runtime"
else
    echo -e "  ${DIM}Runtime — up to date${NC}"
fi

if [ "$BUILD_RUNTIME_ONLY" = true ]; then
    echo ""
    echo -e "${GREEN}Done.${NC}"
    exit 0
fi

# Core capsules
echo ""
echo -e "  ${BOLD}Core capsules:${NC}"
for entry in "${CORE_CAPSULES[@]}"; do
    name="${entry%%:*}"
    dir="${entry#*:}"
    build_capsule "$name" "$dir"
done

# App capsules (only with --all)
if [ "$BUILD_ALL" = true ]; then
    echo ""
    echo -e "  ${BOLD}App capsules:${NC}"
    for entry in "${APP_CAPSULES[@]}"; do
        name="${entry%%:*}"
        dir="${entry#*:}"
        build_capsule "$name" "$dir"
    done
fi

echo ""
echo -e "${GREEN}Done.${NC}"
if [ "$BUILD_ALL" = false ]; then
    echo -e "${DIM}Run with --all to also build chat, notepad, did-provider, ai-provider, llama-provider, agent, ipfs-provider.${NC}"
fi
echo ""
