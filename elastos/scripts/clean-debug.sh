#!/bin/bash
# Clean debug build artifacts to free disk space
# Release binaries are preserved

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET_DIR="$SCRIPT_DIR/../target"

if [ ! -d "$TARGET_DIR/debug" ]; then
    echo "No debug folder found at $TARGET_DIR/debug"
    exit 0
fi

# Calculate size before
SIZE_BEFORE=$(du -sh "$TARGET_DIR/debug" 2>/dev/null | cut -f1)

echo "Cleaning debug build artifacts..."
echo "  Size: $SIZE_BEFORE"

rm -rf "$TARGET_DIR/debug"

# Show new total
if [ -d "$TARGET_DIR" ]; then
    SIZE_AFTER=$(du -sh "$TARGET_DIR" 2>/dev/null | cut -f1)
    echo "  Removed: $SIZE_BEFORE"
    echo "  Target folder now: $SIZE_AFTER"
else
    echo "  Target folder removed entirely"
fi

echo "Done. Release binaries preserved."
