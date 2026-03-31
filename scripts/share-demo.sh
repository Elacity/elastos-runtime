#!/usr/bin/env bash
#
# ElastOS Share Demo
#
# Shares project documentation via the direct `elastos share` flow and prints
# the resulting links.
#
# Prerequisites:
#   - ElastOS built (`cd elastos && cargo build --release -p elastos-server`)
#   - `kubo` installed via `elastos setup --with kubo`
#
# Usage:
#   ./scripts/share-demo.sh                    # Share project docs (multi-doc viewer)
#   ./scripts/share-demo.sh path/to/file.md    # Share a single file
#   ./scripts/share-demo.sh path/to/dir/       # Share a directory of .md files
#

set -euo pipefail

GREEN='\033[0;32m'
CYAN='\033[0;36m'
RED='\033[0;31m'
NC='\033[0m'

# Navigate to project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

source "$(dirname "${BASH_SOURCE[0]}")/resolve-binary.sh"
BINARY="$REPO_ELASTOS_BIN"

# Check binary exists
if [ ! -f "$BINARY" ]; then
    echo -e "${RED}Binary not found at ${BINARY}.${NC} Building..."
    (cd elastos && cargo build --workspace --release)
fi
if [ ! -x "$BINARY" ]; then
    echo -e "${RED}Error:${NC} elastos binary not found at ${BINARY}" >&2
    exit 1
fi

echo -e "Runtime: ${BINARY} ($($BINARY version 2>/dev/null || echo '?'))"

# If a specific path is given, share that
if [ $# -ge 1 ]; then
    echo -e "${CYAN}ElastOS Share Demo${NC}"
    echo -e "Sharing: ${GREEN}$1${NC}"
    echo ""
    $BINARY share "$1"
    exit 0
fi

# Default: bundle project docs into a temp directory and share
DOCS_DIR=$(mktemp -d "${TMPDIR:-/tmp}/elastos-docs-XXXXXX")
trap 'rm -rf "$DOCS_DIR"' EXIT

for f in docs/ARCHITECTURE.md docs/GETTING_STARTED.md docs/OVERVIEW.md README.md ROADMAP.md TASKS.md docs/NOTES.md elastos/CHANGELOG.md; do
    if [ -f "$f" ]; then
        cp "$f" "$DOCS_DIR/"
    fi
done

echo -e "${CYAN}ElastOS Share Demo${NC}"
echo -e "Sharing project documentation..."
echo ""

$BINARY share "$DOCS_DIR"
