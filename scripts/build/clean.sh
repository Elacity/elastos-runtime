#!/usr/bin/env bash
#
# Clean ElastOS build artifacts and caches
#
# Usage:
#   ./scripts/build/clean.sh            # Clean build artifacts only
#   ./scripts/build/clean.sh --all      # Clean artifacts + runtime data + caches
#   ./scripts/build/clean.sh --help     # Show help
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
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

show_help() {
    echo ""
    echo -e "${BOLD}ElastOS Clean${NC}"
    echo ""
    echo -e "${BOLD}Usage:${NC}"
    echo "  ./scripts/build/clean.sh          Clean build artifacts (target/ dirs)"
    echo "  ./scripts/build/clean.sh --all    Clean artifacts + runtime data + caches"
    echo "  ./scripts/build/clean.sh --data   Clean runtime data only (XDG data dir)"
    echo "  ./scripts/build/clean.sh --dry    Show what would be cleaned"
    echo "  ./scripts/build/clean.sh --help   Show this help"
    echo ""
    exit 0
}

show() { echo -e "  ${CYAN}▶${NC} $*"; }
ok()   { echo -e "  ${GREEN}✓${NC} $*"; }

RUNTIME_DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/elastos"
LEGACY_DATA_DIR="$HOME/.elastos"

# ── Parse args ────────────────────────────────────────────────────────

CLEAN_ALL=false
CLEAN_DATA_ONLY=false
DRY_RUN=false

case "${1:-}" in
    --help|-h)  show_help ;;
    --all|-a)   CLEAN_ALL=true ;;
    --data|-d)  CLEAN_DATA_ONLY=true ;;
    --dry)      DRY_RUN=true ;;
    "")         ;;
    *)
        echo -e "${RED}Unknown option:${NC} $1"
        echo "Run ./scripts/build/clean.sh --help for usage."
        exit 1
        ;;
esac

echo ""
echo -e "${BOLD}ElastOS Clean${NC}"
echo ""

# ── Calculate sizes ──────────────────────────────────────────────────

if [ "$DRY_RUN" = true ] || [ "$CLEAN_DATA_ONLY" = false ]; then
    BUILD_DIRS=()
    BUILD_SIZE=0

    # Runtime workspace
    if [ -d "elastos/target" ]; then
        BUILD_DIRS+=("elastos/target")
        s=$(du -sm "elastos/target" 2>/dev/null | cut -f1)
        BUILD_SIZE=$((BUILD_SIZE + s))
    fi

    # Capsule targets
    for dir in elastos/capsules/*/target capsules/*/target; do
        if [ -d "$dir" ]; then
            BUILD_DIRS+=("$dir")
            s=$(du -sm "$dir" 2>/dev/null | cut -f1)
            BUILD_SIZE=$((BUILD_SIZE + s))
        fi
    done

    if [ "$DRY_RUN" = true ]; then
        echo -e "  ${BOLD}Build artifacts:${NC} ${BUILD_SIZE}MB"
        for d in "${BUILD_DIRS[@]}"; do
            s=$(du -sm "$d" 2>/dev/null | cut -f1)
            echo -e "    ${DIM}$d (${s}MB)${NC}"
        done
    fi
fi

if [ "$DRY_RUN" = true ] || [ "$CLEAN_ALL" = true ] || [ "$CLEAN_DATA_ONLY" = true ]; then
    DATA_SIZE=0
    DATA_DIRS=()

    for dir in "$RUNTIME_DATA_DIR" "$LEGACY_DATA_DIR"; do
        if [ -d "$dir" ]; then
            DATA_DIRS+=("$dir")
            s=$(du -sm "$dir" 2>/dev/null | cut -f1)
            DATA_SIZE=$((DATA_SIZE + s))
        fi
    done

    # Temp log files
    for f in /tmp/elastos-demo-serve.log /tmp/elastos-*.log; do
        [ -f "$f" ] && DATA_DIRS+=("$f")
    done

    if [ "$DRY_RUN" = true ]; then
        echo ""
        echo -e "  ${BOLD}Runtime data:${NC} ${DATA_SIZE}MB"
        for d in "${DATA_DIRS[@]}"; do
            echo -e "    ${DIM}$d${NC}"
        done
        echo ""
        echo -e "${DIM}Run without --dry to actually clean.${NC}"
        exit 0
    fi
fi

# ── Clean build artifacts ────────────────────────────────────────────

if [ "$CLEAN_DATA_ONLY" = false ]; then
    show "Cleaning build artifacts..."

    # Runtime workspace
    if [ -d "elastos/target" ]; then
        (cd elastos && cargo clean 2>/dev/null) || true
        ok "elastos/target"
    fi

    # Capsule targets
    for dir in elastos/capsules/*/target capsules/*/target; do
        if [ -d "$dir" ]; then
            rm -rf "$dir"
            ok "$dir"
        fi
    done

    echo -e "  ${GREEN}Freed ~${BUILD_SIZE}MB${NC}"
fi

# ── Clean runtime data ───────────────────────────────────────────────

if [ "$CLEAN_ALL" = true ] || [ "$CLEAN_DATA_ONLY" = true ]; then
    echo ""
    show "Cleaning runtime data..."

    for dir in "$RUNTIME_DATA_DIR" "$LEGACY_DATA_DIR"; do
        if [ -d "$dir" ]; then
            rm -rf "$dir"
            ok "$dir"
        fi
    done

    # Temp files
    rm -f /tmp/elastos-demo-serve.log /tmp/elastos-*.log 2>/dev/null || true
    ok "Temp files"
fi

echo ""
echo -e "${GREEN}Done.${NC}"
echo ""
