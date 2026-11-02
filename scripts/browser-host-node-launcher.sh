#!/bin/sh
set -eu

# Release wrappers locate the admitted files from their installed data root.
bin=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
name=$(basename -- "$0")
case "$name" in
  browser-vm-prepare-rootfs-pool) module="$bin/../scripts/$name.mjs" ;;
  *) module="$bin/$name.mjs" ;;
esac
if [ ! -x "$bin/node" ]; then
  echo "Browser needs its managed Node component. Use Runtime Repair to restore it." >&2
  exit 1
fi
exec "$bin/node" "$module" "$@"
