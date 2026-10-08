#!/usr/bin/env bash
# Package the release binaries already built in this checkout into a
# per-platform tarball: elastos-runtime-<platform>.tar.gz (+ .sha256) in the
# repo root. Run after `cargo build --workspace --release` (CI runs it at the
# tail of the source-home jobs, where setup-source-home has already produced
# most of the artifacts).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

platform() {
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64) printf '%s\n' "linux-amd64" ;;
        Linux-aarch64|Linux-arm64) printf '%s\n' "linux-arm64" ;;
        Darwin-arm64) printf '%s\n' "darwin-arm64" ;;
        *)
            echo "Unsupported release platform: $(uname -s)-$(uname -m)" >&2
            exit 1
            ;;
    esac
}

PLATFORM="$(platform)"
PACKAGE="elastos-runtime-${PLATFORM}"
STAGE_ROOT="$(mktemp -d)"
STAGE="${STAGE_ROOT}/${PACKAGE}"
mkdir -p "${STAGE}"
trap 'rm -rf "${STAGE_ROOT}"' EXIT

# Select binary targets from the elastos workspace and each own-workspace
# capsule. A shared target also contains standalone tools outside this set.
collect() {
    local workspace="$1" names name binary
    names="$(cargo metadata --locked --offline --no-deps --format-version 1 \
        --manifest-path "${workspace}/Cargo.toml" | python3 -c '
import json, sys
metadata = json.load(sys.stdin)
for package in metadata["packages"]:
    if package["id"] in metadata["workspace_members"]:
        for target in package["targets"]:
            if "bin" in target["kind"]:
                print(target["name"])
')"
    while IFS= read -r name; do
        [ -n "$name" ] || continue
        binary="${CARGO_TARGET_DIR:-${workspace}/target}/release/${name}"
        if [ -f "$binary" ] && [ -x "$binary" ]; then
            cp "$binary" "${STAGE}/"
        fi
    done <<< "$names"
}

collect "${ROOT}/elastos"
for lock in "${ROOT}"/capsules/*/Cargo.lock; do
    collect "$(dirname "${lock}")"
done

if [ -z "$(ls -A "${STAGE}")" ]; then
    echo "No release binaries found; run cargo build --workspace --release first." >&2
    exit 1
fi

echo "Packaging ${PACKAGE}:"
ls -l "${STAGE}"

tar -C "${STAGE_ROOT}" -czf "${ROOT}/${PACKAGE}.tar.gz" "${PACKAGE}"
(cd "${ROOT}" && shasum -a 256 "${PACKAGE}.tar.gz" > "${PACKAGE}.tar.gz.sha256")
echo "Wrote ${ROOT}/${PACKAGE}.tar.gz"
