#!/usr/bin/env bash
#
# Fetch model GGUF + llama-server binary for local AI.
#
# Reads models/manifest.json, downloads assets to ~/.local/share/elastos/,
# verifies SHA256 checksums. Skips if already present + checksum matches.
#
# Usage:
#   ./scripts/fetch/fetch-model.sh                  # Fetch default model + llama-server
#   ./scripts/fetch/fetch-model.sh qwen3.5-9b-q4km  # Fetch specific model
#   ./scripts/fetch/fetch-model.sh --list           # List available models
#

set -euo pipefail

BOLD='\033[1m'
DIM='\033[2m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
MANIFEST="$PROJECT_ROOT/models/manifest.json"
INSTALL_DIR="${ELASTOS_DATA_DIR:-$HOME/.local/share/elastos}"
MODEL_DIR="$INSTALL_DIR/models"
BIN_DIR="$INSTALL_DIR/bin"

# ── Helpers ──────────────────────────────────────────────────────────

die()  { echo -e "${RED}Error:${NC} $*" >&2; exit 1; }
info() { echo -e "  ${CYAN}▶${NC} $*"; }
ok()   { echo -e "  ${GREEN}✓${NC} $*"; }
warn() { echo -e "  ${YELLOW}!${NC} $*"; }

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' not found. Please install it."
}

# Read a JSON field by key path. Keys are passed as separate args (avoids dot-in-key issues).
# Usage: json_get manifest.json key1 key2 key3  →  data["key1"]["key2"]["key3"]
json_get() {
    local file="$1"
    shift
    local result
    result=$(python3 -c "
import json, sys
try:
    data = json.load(open(sys.argv[1]))
    val = data
    for k in sys.argv[2:]:
        val = val[k]
    print(val if isinstance(val, str) else json.dumps(val))
except Exception as e:
    print(f'json_get error: {e}', file=sys.stderr)
    sys.exit(1)
" "$file" "$@") || die "Failed to read key path from $file"
    echo "$result"
}

verify_sha256() {
    local file="$1" expected="$2"
    local actual
    actual=$(sha256sum "$file" | cut -d' ' -f1)
    if [ "$actual" != "$expected" ]; then
        die "SHA256 mismatch for $(basename "$file")\n  Expected: $expected\n  Actual:   $actual"
    fi
}

# ── Parse args ───────────────────────────────────────────────────────

if [ ! -f "$MANIFEST" ]; then
    die "Manifest not found: $MANIFEST"
fi

require_cmd python3
require_cmd curl
require_cmd sha256sum

if [ "${1:-}" = "--list" ]; then
    echo -e "\n${BOLD}Available models:${NC}\n"
    python3 -c "
import json, sys
m = json.load(open(sys.argv[1]))
default = m['default_model']
for name, info in m['models'].items():
    tag = ' (default)' if name == default else ''
    print(f'  {name}{tag}')
    print(f'    {info[\"description\"]}')
    print(f'    Size: ~{info[\"size_mb\"]} MB')
    print()
" "$MANIFEST"
    exit 0
fi

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    echo -e "\n${BOLD}Usage:${NC}"
    echo "  ./scripts/fetch/fetch-model.sh                  Fetch default model + llama-server"
    echo "  ./scripts/fetch/fetch-model.sh <model-name>     Fetch specific model"
    echo "  ./scripts/fetch/fetch-model.sh --list           List available models"
    echo ""
    exit 0
fi

MODEL_NAME="${1:-$(json_get "$MANIFEST" default_model)}"

# Validate model name
python3 -c "
import json, sys
m = json.load(open(sys.argv[1]))
name = sys.argv[2]
if name not in m['models']:
    print(f'Unknown model: {name}', file=sys.stderr)
    print(f'Available: {list(m[\"models\"].keys())}', file=sys.stderr)
    sys.exit(1)
" "$MANIFEST" "$MODEL_NAME" || exit 1

echo ""
echo -e "${BOLD}ElastOS Local AI Setup${NC}"
echo ""

# ── Fetch model ──────────────────────────────────────────────────────

MODEL_FILE=$(json_get "$MANIFEST" models "$MODEL_NAME" filename)
MODEL_URL=$(json_get "$MANIFEST" models "$MODEL_NAME" url)
MODEL_SHA=$(json_get "$MANIFEST" models "$MODEL_NAME" sha256)
MODEL_SIZE=$(json_get "$MANIFEST" models "$MODEL_NAME" size_mb)
MODEL_PATH="$MODEL_DIR/$MODEL_FILE"

mkdir -p "$MODEL_DIR"

if [ -f "$MODEL_PATH" ]; then
    info "Verifying existing model: $MODEL_FILE"
    actual_sha=$(sha256sum "$MODEL_PATH" | cut -d' ' -f1)
    if [ "$actual_sha" = "$MODEL_SHA" ]; then
        ok "Model already present and verified: $MODEL_FILE"
    else
        warn "Checksum mismatch — re-downloading"
        rm -f "$MODEL_PATH"
    fi
fi

if [ ! -f "$MODEL_PATH" ]; then
    info "Downloading $MODEL_FILE (~${MODEL_SIZE} MB)..."
    info "URL: $MODEL_URL"
    echo ""
    curl -L --progress-bar -o "$MODEL_PATH.tmp" "$MODEL_URL"
    echo ""
    info "Verifying SHA256..."
    verify_sha256 "$MODEL_PATH.tmp" "$MODEL_SHA"
    mv "$MODEL_PATH.tmp" "$MODEL_PATH"
    ok "Model downloaded and verified: $MODEL_FILE"
fi

# ── Fetch llama-server ───────────────────────────────────────────────

ARCH=$(uname -m)
case "$ARCH" in
    x86_64)  PLATFORM="x86_64-linux" ;;
    aarch64) PLATFORM="aarch64-linux" ;;
    *)       die "Unsupported architecture: $ARCH" ;;
esac

echo ""

# Check if aarch64 needs source build
STRATEGY=$(python3 -c "
import json, sys
m = json.load(open(sys.argv[1]))
p = m.get('llama_server',{}).get('platforms',{}).get(sys.argv[2],{})
print(p.get('strategy',''))
" "$MANIFEST" "$PLATFORM")
if [ "$STRATEGY" = "source-build" ]; then
    LLAMA_BIN="$BIN_DIR/llama-server"
    if [ -f "$LLAMA_BIN" ]; then
        ok "llama-server already present: $LLAMA_BIN"
    else
        warn "No prebuilt llama-server for $ARCH."
        echo -e "  Build from source with: ${BOLD}./scripts/build/build-llama-server.sh${NC}"
        echo ""
        echo -e "  ${DIM}Requires: cmake, gcc, CUDA toolkit (Jetson JetPack provides these)${NC}"
        exit 1
    fi
else
    LLAMA_URL=$(json_get "$MANIFEST" llama_server platforms "$PLATFORM" url)
    LLAMA_SHA=$(json_get "$MANIFEST" llama_server platforms "$PLATFORM" sha256)
    LLAMA_BIN_PATH=$(json_get "$MANIFEST" llama_server platforms "$PLATFORM" binary_path)
    LLAMA_BIN="$BIN_DIR/llama-server"

    mkdir -p "$BIN_DIR"

    if [ -f "$LLAMA_BIN" ]; then
        ok "llama-server already present: $LLAMA_BIN"
    else
        LLAMA_VERSION=$(json_get "$MANIFEST" llama_server version)
        info "Downloading llama-server $LLAMA_VERSION..."
        ARCHIVE="/tmp/llama-server-$LLAMA_VERSION.tar.gz"
        curl -L --progress-bar -o "$ARCHIVE" "$LLAMA_URL"
        echo ""
        info "Verifying SHA256..."
        verify_sha256 "$ARCHIVE" "$LLAMA_SHA"

        info "Extracting llama-server + libraries..."
        # Extract entire release directory (binary + shared libraries)
        LLAMA_EXTRACT_DIR="$BIN_DIR/llama-server-libs"
        rm -rf "$LLAMA_EXTRACT_DIR"
        mkdir -p "$LLAMA_EXTRACT_DIR"
        tar xzf "$ARCHIVE" -C "$LLAMA_EXTRACT_DIR" --strip-components=1
        # Symlink the binary for easy discovery
        ln -sf "$LLAMA_EXTRACT_DIR/llama-server" "$LLAMA_BIN"
        chmod +x "$LLAMA_EXTRACT_DIR/llama-server"
        rm -f "$ARCHIVE"
        ok "llama-server installed: $LLAMA_BIN (+ libs in llama-server-libs/)"
    fi
fi

# ── Summary ──────────────────────────────────────────────────────────

echo ""
echo -e "${GREEN}Setup complete.${NC}"
echo ""
echo -e "  Model:        $MODEL_PATH"
echo -e "  llama-server: $BIN_DIR/llama-server"
echo ""
echo -e "  ${DIM}Start agent: ./scripts/agent.sh --backend local${NC}"
echo ""
