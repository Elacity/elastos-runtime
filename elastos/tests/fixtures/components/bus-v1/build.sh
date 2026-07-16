#!/usr/bin/env bash
set -euo pipefail

FIXTURE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ELASTOS_DIR="$(cd "$FIXTURE_DIR/../../../.." && pwd)"
BUILD_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/elastos-bus-conformance.XXXXXX")"
trap 'rm -rf "$BUILD_ROOT"' EXIT

export CARGO_INCREMENTAL=0
export SOURCE_DATE_EPOCH=0
RUST_SYSROOT="$(rustc --print sysroot)"
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$HOME=/home/builder --remap-path-prefix=$ELASTOS_DIR=/src/elastos --remap-path-prefix=$RUST_SYSROOT=/rust"
CARGO_TARGET_DIR="$BUILD_ROOT/fixture" cargo build \
  --locked \
  --manifest-path "$FIXTURE_DIR/Cargo.toml" \
  --target wasm32-unknown-unknown \
  --release

CARGO_TARGET_DIR="$BUILD_ROOT/componentize" cargo run \
  --quiet \
  --locked \
  --manifest-path "$ELASTOS_DIR/tools/componentize/Cargo.toml" \
  -- \
  "$BUILD_ROOT/fixture/wasm32-unknown-unknown/release/elastos_bus_conformance.wasm" \
  "$FIXTURE_DIR/bus-v1-conformance.component.wasm"

if grep -a -q "$HOME" "$FIXTURE_DIR/bus-v1-conformance.component.wasm"; then
  echo "component artifact contains an unremapped host path" >&2
  exit 1
fi

shasum -a 256 "$FIXTURE_DIR/bus-v1-conformance.component.wasm"
