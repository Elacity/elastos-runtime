#!/usr/bin/env bash
#
# Build llama-server from source (for Jetson/aarch64 with CUDA).
#
# Clones llama.cpp at a pinned tag, builds with CUDA support,
# and installs the binary to ~/.local/share/elastos/bin/llama-server.
#
# Requirements: cmake, gcc/g++, CUDA toolkit (Jetson JetPack provides these)
#
# Usage:
#   ./scripts/build/build-llama-server.sh         # Build with CUDA
#   ./scripts/build/build-llama-server.sh --no-cuda  # Build CPU-only
#

set -euo pipefail

BOLD='\033[1m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
MANIFEST="$PROJECT_ROOT/models/manifest.json"
INSTALL_DIR="${ELASTOS_DATA_DIR:-$HOME/.local/share/elastos}"
BIN_DIR="$INSTALL_DIR/bin"

die()  { echo -e "${RED}Error:${NC} $*" >&2; exit 1; }
info() { echo -e "  ${CYAN}▶${NC} $*"; }
ok()   { echo -e "  ${GREEN}✓${NC} $*"; }

# Read pinned version from manifest
VERSION=$(python3 -c "import json; print(json.load(open('$MANIFEST'))['llama_server']['version'])")

USE_CUDA=true
if [ "${1:-}" = "--no-cuda" ]; then
    USE_CUDA=false
fi

# Check requirements
command -v cmake >/dev/null 2>&1 || die "cmake not found. Install with: sudo apt install cmake"
command -v g++ >/dev/null 2>&1   || die "g++ not found. Install with: sudo apt install g++"

if [ "$USE_CUDA" = true ]; then
    command -v nvcc >/dev/null 2>&1 || die "nvcc not found. Install CUDA toolkit or use --no-cuda"
fi

echo ""
echo -e "${BOLD}Building llama-server from source${NC}"
echo -e "  Version: $VERSION"
echo -e "  CUDA:    $USE_CUDA"
echo ""

BUILD_DIR="/tmp/llama-cpp-build"
rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR"

info "Cloning llama.cpp at tag $VERSION..."
git clone --depth 1 --branch "$VERSION" https://github.com/ggml-org/llama.cpp.git "$BUILD_DIR/llama.cpp"

cd "$BUILD_DIR/llama.cpp"

CMAKE_ARGS="-DCMAKE_BUILD_TYPE=Release"
if [ "$USE_CUDA" = true ]; then
    CMAKE_ARGS="$CMAKE_ARGS -DGGML_CUDA=ON"
fi

info "Configuring..."
cmake -B build $CMAKE_ARGS

info "Building llama-server (this may take several minutes)..."
cmake --build build --target llama-server -j "$(nproc)"

mkdir -p "$BIN_DIR"
cp build/bin/llama-server "$BIN_DIR/llama-server"
chmod +x "$BIN_DIR/llama-server"

# Clean up
rm -rf "$BUILD_DIR"

echo ""
ok "llama-server installed: $BIN_DIR/llama-server"
echo ""
