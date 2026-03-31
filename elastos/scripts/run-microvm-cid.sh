#!/bin/bash
# Run a MicroVM capsule from IPFS CID
# Usage: sudo ./scripts/run-microvm-cid.sh <cid> [port]
#
# Example:
#   sudo ./scripts/run-microvm-cid.sh QmXyz123... 4100

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ELASTOS_DIR="$(dirname "$SCRIPT_DIR")"

CID="${1:-}"
PORT="${2:-4100}"
IPFS_GATEWAY="${IPFS_GATEWAY:-https://ipfs.io}"

if [ -z "$CID" ]; then
    echo "Usage: sudo $0 <cid> [port]"
    echo ""
    echo "Example:"
    echo "  sudo $0 QmXyz123... 4100"
    echo ""
    echo "Environment variables:"
    echo "  IPFS_GATEWAY - Gateway URL (default: https://ipfs.io)"
    exit 1
fi

# Check if running as root (required for crosvm/KVM)
if [ "$EUID" -ne 0 ]; then
    echo "Error: This script must be run as root (sudo)"
    echo ""
    echo "crosvm requires root for KVM access."
    exit 1
fi

echo "============================================"
echo "Running MicroVM Capsule from IPFS"
echo "============================================"
echo ""
echo "CID:      $CID"
echo "Gateway:  $IPFS_GATEWAY"
echo "Port:     $PORT"
echo ""

# Check KVM
if [ ! -e /dev/kvm ]; then
    echo "Error: KVM not available (/dev/kvm not found)"
    echo "MicroVM capsules require hardware virtualization."
    exit 1
fi

# Build elastos if needed
if [ ! -f "$ELASTOS_DIR/target/release/elastos" ]; then
    echo "Building elastos..."
    cd "$ELASTOS_DIR"
    cargo build --release
    echo ""
fi

# Show cache status
CACHE_DIR="${HOME}/.elastos/rootfs-cache"
if [ -d "$CACHE_DIR" ]; then
    CACHE_SIZE=$(du -sh "$CACHE_DIR" 2>/dev/null | cut -f1)
    CACHE_COUNT=$(find "$CACHE_DIR" -maxdepth 1 -name "Qm*" -type f 2>/dev/null | wc -l)
    echo "Cache: $CACHE_DIR ($CACHE_SIZE, $CACHE_COUNT rootfs images)"
else
    echo "Cache: $CACHE_DIR (empty - first run will download rootfs)"
fi
echo ""

# Run
echo "Starting MicroVM..."
echo "Press Ctrl+C to stop."
echo ""

"$ELASTOS_DIR/target/release/elastos" serve \
    --cid "$CID" \
    --ipfs-gateway "$IPFS_GATEWAY" \
    --forward "$PORT:$PORT"
