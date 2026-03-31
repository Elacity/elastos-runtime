#!/bin/bash
# Manage the rootfs cache
# Usage: ./scripts/cache-manage.sh [command]
#
# Commands:
#   status  - Show cache status (default)
#   list    - List cached rootfs images
#   clear   - Clear all cached images
#   size    - Show detailed size info

set -e

CACHE_DIR="${ELASTOS_CACHE_DIR:-$HOME/.elastos/rootfs-cache}"
COMMAND="${1:-status}"

case "$COMMAND" in
    status)
        echo "Rootfs Cache Status"
        echo "==================="
        echo ""
        echo "Location: $CACHE_DIR"

        if [ ! -d "$CACHE_DIR" ]; then
            echo "Status:   Not initialized (empty)"
            exit 0
        fi

        # Count rootfs files (CIDs start with Qm or bafy)
        ROOTFS_COUNT=$(find "$CACHE_DIR" -maxdepth 1 \( -name "Qm*" -o -name "bafy*" \) -type f 2>/dev/null | wc -l)

        # Total size
        TOTAL_SIZE=$(du -sh "$CACHE_DIR" 2>/dev/null | cut -f1)

        # Index info
        if [ -f "$CACHE_DIR/index.json" ]; then
            INDEX_ENTRIES=$(jq '.entries | length' "$CACHE_DIR/index.json" 2>/dev/null || echo "?")
            INDEX_SIZE=$(jq '.total_size' "$CACHE_DIR/index.json" 2>/dev/null || echo "?")
            INDEX_SIZE_MB=$((INDEX_SIZE / 1024 / 1024))
        else
            INDEX_ENTRIES="0"
            INDEX_SIZE_MB="0"
        fi

        echo "Entries:  $ROOTFS_COUNT rootfs images"
        echo "Size:     $TOTAL_SIZE (index reports ${INDEX_SIZE_MB} MB)"
        echo ""
        ;;

    list)
        echo "Cached Rootfs Images"
        echo "===================="
        echo ""

        if [ ! -f "$CACHE_DIR/index.json" ]; then
            echo "No cache index found."
            exit 0
        fi

        jq -r '.entries | to_entries[] | "\(.value.cid)\t\(.value.size / 1024 / 1024 | floor) MB\t\(.value.last_accessed | todate)"' \
            "$CACHE_DIR/index.json" 2>/dev/null | column -t -s $'\t' || echo "No entries"
        echo ""
        ;;

    clear)
        echo "Clear Rootfs Cache"
        echo "=================="
        echo ""

        if [ ! -d "$CACHE_DIR" ]; then
            echo "Cache directory does not exist."
            exit 0
        fi

        # Show what will be deleted
        TOTAL_SIZE=$(du -sh "$CACHE_DIR" 2>/dev/null | cut -f1)
        echo "This will delete: $TOTAL_SIZE"
        echo "Location: $CACHE_DIR"
        echo ""

        read -p "Are you sure? [y/N] " -n 1 -r
        echo ""

        if [[ $REPLY =~ ^[Yy]$ ]]; then
            rm -rf "$CACHE_DIR"
            echo "Cache cleared."
        else
            echo "Cancelled."
        fi
        ;;

    size)
        echo "Cache Size Details"
        echo "=================="
        echo ""

        if [ ! -d "$CACHE_DIR" ]; then
            echo "Cache directory does not exist."
            exit 0
        fi

        echo "Directory: $CACHE_DIR"
        echo ""

        # List all files with sizes
        echo "Files:"
        ls -lhS "$CACHE_DIR" 2>/dev/null | grep -v "^total" | head -20

        echo ""
        echo "Total: $(du -sh "$CACHE_DIR" 2>/dev/null | cut -f1)"
        ;;

    *)
        echo "Usage: $0 [command]"
        echo ""
        echo "Commands:"
        echo "  status  - Show cache status (default)"
        echo "  list    - List cached rootfs images"
        echo "  clear   - Clear all cached images"
        echo "  size    - Show detailed size info"
        exit 1
        ;;
esac
