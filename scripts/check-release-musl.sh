#!/usr/bin/env bash
# Type-check the native Rust producers selected by release preparation.
# Usage: check-release-musl.sh TARGET [check|build]
# build reproduces N's locked release compilation, including support binaries.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
source scripts/publish-release.sh

target=${1:?usage: check-release-musl.sh TARGET [check|build]}
mode=${2:-check}
case "$target" in
    x86_64-unknown-linux-musl) platform=linux-amd64 ;;
    aarch64-unknown-linux-musl) platform=linux-arm64 ;;
    *) die "Unsupported musl target: $target" ;;
esac
case "$mode" in check|build) ;; *) die "Expected check or build" ;; esac
rustup target add "$target"
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
started=$SECONDS
(cd elastos && cargo "$mode" --locked --release --target "$target" -p elastos-server --bin elastos)

# Use the release producer's list and platform resolution, including omissions.
selected=$(python3 - "$platform" "${SUPPORT_BINARY_ASSETS[@]}" home-cli <<'PY'
import importlib.util
import json
from pathlib import Path
import sys

spec = importlib.util.spec_from_file_location("integrity", "scripts/components-release-integrity-check.py")
integrity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(integrity)
external = json.loads(Path("components.json").read_text())["external"]
for name in dict.fromkeys(sys.argv[2:]):
    _, info = integrity.resolve_platform_info(external[name], sys.argv[1])
    if info is not None:
        print(name)
PY
)
while IFS= read -r name; do
    directory=$(resolve_capsule_dir "$name")
    (cd "$directory" && cargo "$mode" --locked --release --target "$target")
done <<< "$selected"

# This native Browser owner is a separate producer after the support list.
if jq -e --arg platform "$platform" \
    '.external["browser-vz-engine-supervisor"].platforms[$platform]' components.json >/dev/null; then
    (cd elastos && cargo "$mode" --locked --release -p elastos-vz \
        --bin browser-vz-engine-supervisor --target "$target")
fi
echo "musl $mode PASS: $target ($((SECONDS - started))s)"
