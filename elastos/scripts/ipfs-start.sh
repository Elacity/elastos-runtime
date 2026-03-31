#!/bin/bash
# Start IPFS daemon if not running
# Usage: ./scripts/ipfs-start.sh

set -e

# Check if IPFS is installed
if ! command -v ipfs &> /dev/null; then
    echo "IPFS is not installed."
    echo ""
    echo "Install with:"
    echo "  # Download"
    echo "  wget https://dist.ipfs.tech/kubo/v0.27.0/kubo_v0.27.0_linux-amd64.tar.gz"
    echo "  tar -xvzf kubo_v0.27.0_linux-amd64.tar.gz"
    echo "  cd kubo"
    echo "  sudo bash install.sh"
    echo ""
    echo "  # Initialize"
    echo "  ipfs init"
    exit 1
fi

# Function to check if IPFS API is responding
ipfs_api_ready() {
    curl -s -X POST http://localhost:5001/api/v0/id > /dev/null 2>&1
}

# Check if IPFS daemon is already running
if ipfs_api_ready; then
    echo "IPFS daemon is already running."
    curl -s -X POST http://localhost:5001/api/v0/id | grep -oE '"(ID|AgentVersion)":"[^"]*"' | tr ',' '\n'
    exit 0
fi

# Check if IPFS is initialized
if [ ! -d ~/.ipfs ]; then
    echo "Initializing IPFS..."
    ipfs init
fi

# Configure gateway to avoid port conflicts
GATEWAY_PORT=$(ipfs config Addresses.Gateway 2>/dev/null | grep -oE '[0-9]+$' || echo "8080")
if [ "$GATEWAY_PORT" = "8080" ]; then
    # Check if 8080 is in use
    if lsof -i :8080 > /dev/null 2>&1 || netstat -tuln 2>/dev/null | grep -q ':8080 '; then
        echo "Port 8080 in use, configuring IPFS gateway to use 8081..."
        ipfs config Addresses.Gateway /ip4/127.0.0.1/tcp/8081
    fi
fi

echo "Starting IPFS daemon..."
echo "This will run in the background. Use 'ipfs shutdown' to stop."
echo ""

# Start daemon in background
nohup ipfs daemon > /tmp/ipfs-daemon.log 2>&1 &
IPFS_PID=$!

echo "IPFS daemon starting (PID: $IPFS_PID)..."
echo "Log: /tmp/ipfs-daemon.log"

# Wait for daemon to be ready (check API endpoint)
for i in {1..30}; do
    if ipfs_api_ready; then
        echo ""
        echo "IPFS daemon is ready!"
        curl -s -X POST http://localhost:5001/api/v0/id | grep -oE '"(ID|AgentVersion)":"[^"]*"' | tr ',' '\n'
        echo ""
        echo "API: http://localhost:5001"
        exit 0
    fi
    sleep 1
    echo -n "."
done

echo ""
echo "IPFS daemon failed to start. Check /tmp/ipfs-daemon.log"
echo ""
tail -20 /tmp/ipfs-daemon.log
exit 1
