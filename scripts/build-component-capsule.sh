#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_BIN="${CARGO_BIN:-cargo}"
RUSTC_BIN="${RUSTC_BIN:-rustc}"
CAPSULE_DIR="${1:-}"

if [[ -z "$CAPSULE_DIR" ]]; then
    echo "usage: $0 <capsule-directory>" >&2
    exit 2
fi

CAPSULE_DIR="$(cd "$CAPSULE_DIR" && pwd)"
MANIFEST="$CAPSULE_DIR/capsule.json"
CARGO_MANIFEST="$CAPSULE_DIR/Cargo.toml"
[[ -f "$CARGO_MANIFEST" ]] || { echo "missing $CARGO_MANIFEST" >&2; exit 1; }
[[ -f "$MANIFEST" ]] || { echo "missing $MANIFEST" >&2; exit 1; }

ENTRYPOINT="$(python3 - "$MANIFEST" "$ROOT/elastos/wit/elastos-bus-v1.wit" <<'PY'
import hashlib
import json
import pathlib
import sys

manifest_path = pathlib.Path(sys.argv[1])
wit_path = pathlib.Path(sys.argv[2])
manifest = json.loads(manifest_path.read_text(encoding="utf-8"))

if manifest.get("runtime_abi") != "elastos.component/v1":
    raise SystemExit("manifest runtime_abi must be elastos.component/v1")
if manifest.get("bus_contract") != "elastos:bus@v1":
    raise SystemExit("manifest bus_contract must be elastos:bus@v1")
if manifest.get("execution") != "component":
    raise SystemExit("manifest execution must be component")

entrypoint = manifest.get("entrypoint")
if not isinstance(entrypoint, str) or not entrypoint.endswith(".component.wasm"):
    raise SystemExit("component manifest must name a .component.wasm entrypoint")
if pathlib.PurePosixPath(entrypoint).is_absolute() or ".." in pathlib.PurePosixPath(entrypoint).parts:
    raise SystemExit("component entrypoint must stay within the capsule directory")

wit_hash = hashlib.sha256(wit_path.read_bytes()).hexdigest()
if manifest.get("wit_world_sha256") != wit_hash:
    raise SystemExit("manifest wit_world_sha256 does not match elastos-bus-v1.wit")

print(entrypoint)
PY
)"

TARGET_NAME="$($CARGO_BIN metadata \
    --no-deps \
    --format-version 1 \
    --manifest-path "$CARGO_MANIFEST" \
    | python3 -c '
import json
import pathlib
import sys

manifest = pathlib.Path(sys.argv[1]).resolve()
packages = [
    package
    for package in json.load(sys.stdin)["packages"]
    if pathlib.Path(package["manifest_path"]).resolve() == manifest
]
if len(packages) != 1:
    raise SystemExit("component Cargo package not found in metadata")
targets = [
    target for target in packages[0]["targets"] if "cdylib" in target["crate_types"]
]
if len(targets) != 1:
    raise SystemExit("component crate must expose exactly one cdylib target")
print(targets[0]["name"].replace("-", "_"))
' "$CARGO_MANIFEST")"

TARGET="wasm32-unknown-unknown"
BUILD_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/elastos-component-build.XXXXXX")"
trap 'rm -rf "$BUILD_ROOT"' EXIT

RUST_SYSROOT="$($RUSTC_BIN --print sysroot)"
REMAP_FLAGS="--remap-path-prefix=$HOME=/home/builder"
REMAP_FLAGS+=" --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo"
REMAP_FLAGS+=" --remap-path-prefix=$ROOT=/src/elastos-runtime"
REMAP_FLAGS+=" --remap-path-prefix=$RUST_SYSROOT=/rust"

export CARGO_INCREMENTAL=0
export SOURCE_DATE_EPOCH=0
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }$REMAP_FLAGS"

CARGO_TARGET_DIR="$BUILD_ROOT/capsule" "$CARGO_BIN" build \
    --locked \
    --manifest-path "$CARGO_MANIFEST" \
    --lib \
    --target "$TARGET" \
    --release

CORE_WASM="$BUILD_ROOT/capsule/$TARGET/release/$TARGET_NAME.wasm"
[[ -f "$CORE_WASM" ]] || { echo "missing built core module $CORE_WASM" >&2; exit 1; }

mkdir -p "$CAPSULE_DIR/$(dirname "$ENTRYPOINT")"
CARGO_TARGET_DIR="$BUILD_ROOT/componentize" "$CARGO_BIN" run \
    --quiet \
    --locked \
    --manifest-path "$ROOT/elastos/tools/componentize/Cargo.toml" \
    -- "$CORE_WASM" "$CAPSULE_DIR/$ENTRYPOINT"

python3 - "$CAPSULE_DIR/$ENTRYPOINT" "$ROOT" "$HOME" <<'PY'
import hashlib
import pathlib
import sys

artifact = pathlib.Path(sys.argv[1])
data = artifact.read_bytes()
for forbidden in (sys.argv[2].encode(), sys.argv[3].encode()):
    if forbidden and forbidden in data:
        raise SystemExit(f"{artifact} contains an unremapped host path")
print(f"{hashlib.sha256(data).hexdigest()}  {artifact}")
PY
