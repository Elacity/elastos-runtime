#!/usr/bin/env bash
#
# Build/install crosvm and install vmlinux for ElastOS supervisor runtime.
#
# This script installs:
#   ~/.local/share/elastos/bin/crosvm
#   ~/.local/share/elastos/bin/vmlinux
#
# Usage:
#   ./scripts/setup-crosvm.sh
#   ./scripts/setup-crosvm.sh --install-deps
#   ./scripts/setup-crosvm.sh --kernel /path/to/vmlinux
#   ./scripts/setup-crosvm.sh --source-dir ~/.cache/elastos/crosvm
#

set -euo pipefail

BOLD='\033[1m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
NC='\033[0m'

info() { echo -e "${GREEN}▶${NC} $*"; }
warn() { echo -e "${YELLOW}!${NC} $*"; }
die()  { echo -e "${RED}Error:${NC} $*" >&2; exit 1; }

validate_kernel() {
    local kernel="$1"
    local file_desc
    file_desc="$(file -b "$kernel" 2>/dev/null || true)"

    case "$(uname -m)" in
        x86_64)
            if [[ "$file_desc" == *"Linux kernel x86 boot executable bzImage"* ]]; then
                return 0
            fi
            ;;
        aarch64|arm64)
            if [[ "$file_desc" == *"Linux kernel ARM64 boot executable Image"* ]]; then
                return 0
            fi
            ;;
    esac

    strings -a "$kernel" | grep -q "ext4" || die "Kernel missing ext4 support marker: $kernel"
    strings -a "$kernel" | grep -q "virtio_blk" || die "Kernel missing virtio_blk support marker: $kernel"
    strings -a "$kernel" | grep -q "virtio_pci" || die "Kernel missing virtio_pci support marker: $kernel"
}

kernel_arch() {
    case "$(uname -m)" in
        x86_64) echo "x86_64" ;;
        aarch64|arm64) echo "aarch64" ;;
        *) die "Unsupported host architecture for kernel fetch: $(uname -m)" ;;
    esac
}

kernel_default_url() {
    local arch="$1"
    echo "https://storage.googleapis.com/crosvm/integration_tests/guest-bzimage-${arch}-r0016"
}

fetch_kernel() {
    local arch="$1"
    local out="$2"
    local url

    # The upstream crosvm test kernel (guest-bzimage-aarch64-r0016) is broken on
    # GICv3-only hosts like Jetson Orin. On aarch64, prefer /boot/Image if available.
    if [[ "$arch" == "aarch64" && -f /boot/Image ]]; then
        warn "Using /boot/Image as guest kernel (upstream crosvm aarch64 kernel is broken on GICv3-only hosts)"
        cp /boot/Image "$out"
        validate_kernel "$out"
        return
    fi

    url="$(kernel_default_url "$arch")"
    info "Downloading official crosvm guest kernel ($arch)"
    curl -fL --retry 3 -o "$out" "$url"
    validate_kernel "$out"
}

show_help() {
    cat <<EOF
ElastOS crosvm setup

Installs crosvm + vmlinux under ~/.local/share/elastos/bin for supervisor VM launches.
If no --kernel is provided, the script fetches the official crosvm guest kernel for this host architecture.

Usage:
  ./scripts/setup-crosvm.sh [options]

Options:
  --source-dir PATH   crosvm source checkout (default: ~/.cache/elastos/crosvm)
  --kernel PATH       guest kernel image to install as vmlinux
  --install-deps      run crosvm's Debian/Ubuntu dependency setup (tools/setup)
  --skip-build        do not build crosvm (expects built binary in source dir)
  --help              show this help
EOF
}

SOURCE_DIR="${HOME}/.cache/elastos/crosvm"
KERNEL_SRC=""
INSTALL_DEPS=false
SKIP_BUILD=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --source-dir)
            [[ -n "${2:-}" ]] || die "Usage: --source-dir PATH"
            SOURCE_DIR="$2"; shift 2 ;;
        --kernel)
            [[ -n "${2:-}" ]] || die "Usage: --kernel /path/to/vmlinux"
            KERNEL_SRC="$2"; shift 2 ;;
        --install-deps) INSTALL_DEPS=true; shift ;;
        --skip-build) SKIP_BUILD=true; shift ;;
        --help|-h) show_help; exit 0 ;;
        *) die "Unknown option: $1" ;;
    esac
done

for cmd in git cargo curl file; do
    command -v "$cmd" >/dev/null 2>&1 || die "Missing required tool: $cmd"
done

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${HOME}/.local/share/elastos/bin"
mkdir -p "$BIN_DIR"

if [[ ! -d "$SOURCE_DIR/.git" ]]; then
    info "Cloning crosvm source into $SOURCE_DIR"
    mkdir -p "$(dirname "$SOURCE_DIR")"
    git clone https://chromium.googlesource.com/crosvm/crosvm "$SOURCE_DIR"
else
    info "Using existing crosvm source: $SOURCE_DIR"
fi

pushd "$SOURCE_DIR" >/dev/null

info "Updating submodules"
git submodule update --init --recursive

if [[ "$INSTALL_DEPS" == true ]]; then
    if [[ -x "./tools/setup" ]]; then
        info "Running crosvm dependency setup (may prompt for sudo)"
        ./tools/setup
    else
        warn "tools/setup not found in crosvm source; skipping dependency install"
    fi
fi

if [[ "$SKIP_BUILD" == false ]]; then
    info "Building crosvm (release)"
    cargo build --release
fi

CROSVM_BIN="$SOURCE_DIR/target/release/crosvm"
[[ -x "$CROSVM_BIN" ]] || die "Built crosvm binary not found at $CROSVM_BIN"

info "Installing crosvm -> $BIN_DIR/crosvm"
install -m 755 "$CROSVM_BIN" "$BIN_DIR/crosvm"

if [[ -n "$KERNEL_SRC" ]]; then
    [[ -f "$KERNEL_SRC" ]] || die "Kernel file not found: $KERNEL_SRC"
    validate_kernel "$KERNEL_SRC"
    info "Installing vmlinux -> $BIN_DIR/vmlinux"
    install -m 644 "$KERNEL_SRC" "$BIN_DIR/vmlinux"
else
    if [[ -f "$BIN_DIR/vmlinux" ]]; then
        validate_kernel "$BIN_DIR/vmlinux"
        info "Keeping existing vmlinux at $BIN_DIR/vmlinux"
    else
        tmp_kernel="$(mktemp)"
        fetch_kernel "$(kernel_arch)" "$tmp_kernel"
        info "Installing vmlinux -> $BIN_DIR/vmlinux"
        install -m 644 "$tmp_kernel" "$BIN_DIR/vmlinux"
        rm -f "$tmp_kernel"
    fi
fi

popd >/dev/null

echo ""
echo -e "${BOLD}Done${NC}"
echo "  crosvm:  $BIN_DIR/crosvm"
echo "  vmlinux: $BIN_DIR/vmlinux"
echo ""
echo "Verify:"
echo "  test -x $BIN_DIR/crosvm && echo crosvm_ok"
echo "  test -f $BIN_DIR/vmlinux && echo vmlinux_ok"
