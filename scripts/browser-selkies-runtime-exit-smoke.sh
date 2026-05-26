#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"

cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

"$repo_root/scripts/browser-selkies-runtime-exit-target.sh" \
  --out-dir "$tmp_dir/target" \
  --allowed-hosts example.com \
  --verify-url https://example.com/ \
  --verify \
  --cleanup-after-verify \
  "$@"
