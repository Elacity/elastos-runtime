#!/usr/bin/env bash
# scripts/dev/sign-elastos-vz/sign.sh
#
# Phase 2 Day 4 — local-development helper that bestows the
# `com.apple.security.virtualization` entitlement on a freshly
# built `elastos` binary so Apple's
# `VZVirtualMachineConfiguration.validateWithError` no longer
# rejects it, unblocking the Day 3 lifecycle wiring.
#
# This script is for local Apple Silicon development only.
# Phase 6 will replace it with a proper developer-certificate
# signing flow + notarization for distribution builds.
#
# Usage:
#   scripts/dev/sign-elastos-vz/sign.sh                        # signs target/debug/elastos
#   scripts/dev/sign-elastos-vz/sign.sh target/release/elastos # explicit binary path
#   scripts/dev/sign-elastos-vz/sign.sh --verify-only          # show the current entitlements
#
# Exit codes:
#   0  — binary now carries the entitlement
#   1  — binary is missing or signing failed
#   2  — wrong OS / wrong architecture
#
# Anchors:
#   - docs/vz-backend/PLAN.md (Phase 2 Day 4)
#   - elastos-vz/src/ffi/lifecycle.rs::ENTITLEMENT_HINT
#   - docs/MAC.md (operator recipe)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENTITLEMENTS="$SCRIPT_DIR/vz.entitlements.plist"
# Cargo workspace root lives at <repo>/elastos/, so the debug
# build lands at <repo>/elastos/target/debug/elastos — NOT at
# <repo>/target/debug/elastos. Day 4 documented this in MAC.md
# but the default path here pointed at the repo-root build dir,
# which only exists if someone runs cargo from the repo root.
DEFAULT_BINARY="elastos/target/debug/elastos"

require_macos() {
  if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "error: sign.sh only runs on macOS (got $(uname -s))." >&2
    echo "       The com.apple.security.virtualization entitlement" >&2
    echo "       has no meaning outside Apple's platforms." >&2
    exit 2
  fi
}

usage() {
  cat <<USAGE
Usage: $0 [BINARY_PATH] [--verify-only]

Signs BINARY_PATH (default: $DEFAULT_BINARY) with the
com.apple.security.virtualization entitlement using ad-hoc
signing (-s -). Re-run after every \`cargo build\` — codesign
does not survive a relink.

With --verify-only, reports the currently-applied entitlements
without re-signing.
USAGE
}

main() {
  require_macos

  local binary=""
  local verify_only=0

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --verify-only) verify_only=1; shift ;;
      -h|--help)     usage; exit 0 ;;
      --*)           echo "error: unknown flag '$1'" >&2; usage >&2; exit 1 ;;
      *)             binary="$1"; shift ;;
    esac
  done

  if [[ -z "$binary" ]]; then
    # Resolve relative to the repo root, not to the script
    # location, so a contributor running from any cwd gets a
    # sensible default.
    local repo_root
    repo_root="$(cd "$SCRIPT_DIR/../../.." && pwd)"
    binary="$repo_root/$DEFAULT_BINARY"
  fi

  if [[ ! -f "$binary" ]]; then
    echo "error: binary not found: $binary" >&2
    echo "       run \`cargo build\` first." >&2
    exit 1
  fi

  if [[ $verify_only -eq 1 ]]; then
    echo "Current entitlements on $binary:"
    codesign --display --entitlements - "$binary" || true
    exit 0
  fi

  echo "Signing $binary with $ENTITLEMENTS"
  echo "  identity: - (ad-hoc; local development only)"
  echo

  # `--force` re-signs in place; `--sign -` requests ad-hoc
  # signing (no developer certificate needed); `--entitlements`
  # bakes the plist into the signature.
  codesign --force \
    --sign - \
    --entitlements "$ENTITLEMENTS" \
    --options runtime \
    "$binary"

  echo
  echo "Verifying entitlements were applied..."
  if ! codesign --display --entitlements - "$binary" 2>&1 | grep -q "com.apple.security.virtualization"; then
    echo "error: signing reported success but the virtualization entitlement is missing." >&2
    echo "       inspect with: codesign --display --entitlements - $binary" >&2
    exit 1
  fi

  echo
  echo "Done. \`$binary\` can now drive Apple's Virtualization.framework."
  echo "Re-run this script after every \`cargo build\` — codesign does not survive a relink."
}

main "$@"
