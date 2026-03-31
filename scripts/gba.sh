#!/usr/bin/env bash
#
# ElastOS GBA Emulator
#
# Runs the Game Boy Advance emulator as an ElastOS data capsule.
# Lists available ROM capsules and lets you pick one, or runs the
# standalone emulator for drag-and-drop. When tunnel-provider and
# cloudflared are installed, `elastos run` prints a public preview URL
# so the browser viewer is reachable off-box as well.
#
# Usage:
#   ./scripts/gba.sh                           # Interactive — pick a game
#   ./scripts/gba.sh capsules/gba-ucity      # Run specific ROM capsule
#   ./scripts/gba.sh --standalone               # Standalone emulator (drag-and-drop)
#   ./scripts/gba.sh --help                     # Show help
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
cd "$(dirname "${BASH_SOURCE[0]}")/.."

source "$(dirname "${BASH_SOURCE[0]}")/resolve-binary.sh"
ELASTOS="$REPO_ELASTOS_BIN"
runtime_version() { "$ELASTOS" --version 2>/dev/null | head -1; }

# ── Help ──────────────────────────────────────────────────────────────

show_help() {
    echo ""
    echo -e "${BOLD}ElastOS GBA Emulator${NC}"
    echo ""
    echo "  Runs Game Boy Advance games as ElastOS data capsules."
    echo "  Uses mGBA compiled to WebAssembly, served through the runtime."
    echo "  Save states persist to ElastOS capability-gated storage."
    echo "  When tunnel-provider + cloudflared are installed, launch prints"
    echo "  a public preview URL so the viewer is reachable from another device."
    echo ""
    echo -e "${BOLD}Usage:${NC}"
    echo "  ./scripts/gba.sh                            Interactive game picker"
    echo "  ./scripts/gba.sh capsules/gba-ucity       Run specific ROM capsule"
    echo "  ./scripts/gba.sh --standalone                Emulator only (drag-and-drop ROMs)"
    echo "  ./scripts/gba.sh --list                      List available games"
    echo "  ./scripts/gba.sh --help                      Show this help"
    echo ""
    echo -e "${BOLD}Controls:${NC}"
    echo "  Arrow keys     D-pad"
    echo "  Z              A button"
    echo "  X              B button"
    echo "  Enter          Start"
    echo "  Backspace      Select"
    echo "  A / S          L / R shoulder"
    echo ""
    echo -e "${BOLD}Adding ROMs:${NC}"
    echo "  1. Create a directory:  mkdir capsules/my-game"
    echo "  2. Copy your ROM:      cp game.gba capsules/my-game/rom.gba"
    echo "  3. Create capsule.json:"
    echo '     {"version":"1.0","name":"my-game","type":"data",'
    echo '      "entrypoint":"rom.gba","viewer":"../gba-emulator",'
    echo '      "permissions":{"storage":["localhost://Users/self/.AppData/LocalHost/GBA/my-game/*"]}}'
    echo "  4. Run:                ./scripts/gba.sh capsules/my-game"
    echo ""
    exit 0
}

# ── Discover ROM capsules ─────────────────────────────────────────────

GAMES=()
GAME_NAMES=()
GAME_HAS_ROM=()

discover_games() {
    GAMES=()
    GAME_NAMES=()
    GAME_HAS_ROM=()

    for dir in capsules/*/; do
        [ -f "$dir/capsule.json" ] || continue

        # Check if this is a GBA data capsule (has viewer pointing to gba-emulator)
        local viewer
        viewer=$(grep -o '"viewer": "[^"]*"' "$dir/capsule.json" 2>/dev/null | cut -d'"' -f4 || true)
        [ -z "$viewer" ] && continue
        [[ "$viewer" == *"gba-emulator"* ]] || continue

        local name
        name=$(grep -o '"name": "[^"]*"' "$dir/capsule.json" | cut -d'"' -f4)

        local entrypoint
        entrypoint=$(grep -o '"entrypoint": "[^"]*"' "$dir/capsule.json" | cut -d'"' -f4)

        local rom_exists="no"
        [ -f "$dir/$entrypoint" ] && rom_exists="yes"

        GAMES+=("${dir%/}")
        GAME_NAMES+=("$name")
        GAME_HAS_ROM+=("$rom_exists")
    done
}

list_games() {
    discover_games

    echo ""
    echo -e "${BOLD}Available GBA Games${NC}"
    echo ""

    if [ ${#GAMES[@]} -eq 0 ]; then
        echo -e "  ${DIM}No ROM capsules found.${NC}"
        echo -e "  ${DIM}Run ./scripts/gba.sh --help to learn how to add ROMs.${NC}"
    else
        local status
        for i in "${!GAMES[@]}"; do
            status="${GREEN}ready${NC}"
            if [ "${GAME_HAS_ROM[$i]}" = "no" ]; then
                status="${YELLOW}ROM missing${NC}"
            fi
            printf "  %-25s %b\n" "${GAME_NAMES[$i]}" "$status"
        done
    fi

    echo ""
    echo -e "  ${DIM}Standalone emulator (drag-and-drop): ./scripts/gba.sh --standalone${NC}"
    echo ""
}

# ── Parse args ────────────────────────────────────────────────────────

TARGET=""
INTERACTIVE=true

case "${1:-}" in
    --help|-h)
        show_help
        ;;
    --list|-l)
        list_games
        exit 0
        ;;
    --standalone)
        TARGET="capsules/gba-emulator"
        INTERACTIVE=false
        ;;
    "")
        INTERACTIVE=true
        ;;
    *)
        TARGET="$1"
        INTERACTIVE=false
        ;;
esac

# ── Interactive game picker ───────────────────────────────────────────

if [ "$INTERACTIVE" = true ]; then
    discover_games

    echo ""
    echo -e "${BOLD}ElastOS GBA Emulator${NC}"
    echo ""

    # Build the menu
    OPTIONS=()
    OPTIONS+=("Standalone emulator (drag-and-drop ROMs)")

    label=""
    for i in "${!GAMES[@]}"; do
        label="${GAME_NAMES[$i]}"
        if [ "${GAME_HAS_ROM[$i]}" = "no" ]; then
            label="$label (ROM missing — place file first)"
        fi
        OPTIONS+=("$label")
    done

    # Display numbered menu
    num=0
    game_idx=0
    for i in "${!OPTIONS[@]}"; do
        num=$((i + 1))
        if [ $i -eq 0 ]; then
            echo -e "  ${BOLD}$num)${NC} ${OPTIONS[$i]}"
        else
            game_idx=$((i - 1))
            if [ "${GAME_HAS_ROM[$game_idx]}" = "no" ]; then
                echo -e "  ${DIM}$num) ${OPTIONS[$i]}${NC}"
            else
                echo -e "  ${BOLD}$num)${NC} ${OPTIONS[$i]}"
            fi
        fi
    done

    echo ""
    echo -ne "${CYAN}?${NC} ${BOLD}Pick a game${NC} ${DIM}(1-${#OPTIONS[@]}, default: 1)${NC}: "
    read -r choice

    # Default to 1 (standalone)
    choice="${choice:-1}"

    if ! [[ "$choice" =~ ^[0-9]+$ ]] || [ "$choice" -lt 1 ] || [ "$choice" -gt "${#OPTIONS[@]}" ]; then
        echo -e "${RED}Invalid choice.${NC}"
        exit 1
    fi

    if [ "$choice" -eq 1 ]; then
        TARGET="capsules/gba-emulator"
    else
        game_idx=$((choice - 2))
        TARGET="${GAMES[$game_idx]}"

        # Check ROM exists
        if [ "${GAME_HAS_ROM[$game_idx]}" = "no" ]; then
            entrypoint=$(grep -o '"entrypoint": "[^"]*"' "$TARGET/capsule.json" | cut -d'"' -f4)
            echo ""
            echo -e "${YELLOW}ROM file missing:${NC} $TARGET/$entrypoint"
            echo -e "Place the ROM file there and try again."
            exit 1
        fi
    fi

    echo ""
fi

# ── Validate target ──────────────────────────────────────────────────

if [ ! -d "$TARGET" ]; then
    echo -e "${RED}Capsule not found:${NC} $TARGET"
    echo ""
    echo "Available capsules:"
    list_games
    exit 1
fi

if [ ! -f "$TARGET/capsule.json" ]; then
    echo -e "${RED}No capsule.json in:${NC} $TARGET"
    exit 1
fi

# For data capsules, check ROM exists
if grep -q '"type": "data"' "$TARGET/capsule.json" 2>/dev/null; then
    entrypoint=$(grep -o '"entrypoint": "[^"]*"' "$TARGET/capsule.json" | cut -d'"' -f4)
    if [ -n "$entrypoint" ] && [ "$entrypoint" != "index.html" ] && [ ! -f "$TARGET/$entrypoint" ]; then
        echo -e "${YELLOW}ROM file missing:${NC} $TARGET/$entrypoint"
        echo ""
        echo "Place the ROM file at: $TARGET/$entrypoint"
        exit 1
    fi
fi

# ── Build runtime ────────────────────────────────────────────────────

if [ ! -x "$ELASTOS" ] || [ "$(find elastos/crates/ -name '*.rs' -newer "$ELASTOS" 2>/dev/null | head -1)" ]; then
    echo -e "  ${CYAN}▶${NC} Building runtime..."
    (cd elastos && cargo build -p elastos-server --release 2>/dev/null)
fi

[[ -x "$ELASTOS" ]] || { echo -e "${RED}Error:${NC} repo elastos binary not found at ${REPO_ELASTOS_BIN}" >&2; exit 1; }

# ── Launch ────────────────────────────────────────────────────────────

CAPSULE_NAME=$(grep -o '"name": "[^"]*"' "$TARGET/capsule.json" | cut -d'"' -f4)
echo -e "${DIM}Runtime: ${ELASTOS} (${YELLOW}$(runtime_version)${DIM})${NC}"
echo -e "${GREEN}Launching ${BOLD}$CAPSULE_NAME${NC}${GREEN}...${NC}"

exec "$ELASTOS" run "$TARGET"
