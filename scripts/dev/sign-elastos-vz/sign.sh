#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENTITLEMENTS="${SCRIPT_DIR}/entitlements.plist"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "sign-elastos-vz requires macOS" >&2
  exit 1
fi

if [[ $# -eq 0 ]]; then
  echo "usage: $0 /path/to/browser-vz-engine-supervisor [...]" >&2
  exit 2
fi

for binary in "$@"; do
  if [[ ! -f "$binary" || ! -x "$binary" ]]; then
    echo "not an executable file: ${binary}" >&2
    exit 1
  fi

  /usr/bin/codesign --force --sign - --entitlements "$ENTITLEMENTS" "$binary"
  /usr/bin/codesign --verify --strict "$binary"
  if ! /usr/bin/codesign -d --entitlements :- "$binary" 2>/dev/null |
    grep -q "com.apple.security.virtualization"; then
    echo "missing com.apple.security.virtualization after signing: ${binary}" >&2
    exit 1
  fi
done
