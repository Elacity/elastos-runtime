#!/usr/bin/env bash
# Package the release binaries already built in this checkout into a
# per-platform tarball: elastos-runtime-<platform>.tar.gz (+ .sha256) in the
# repo root. Run after `cargo build --workspace --release` (CI runs it at the
# tail of the source-home jobs, where setup-source-home has already produced
# most of the artifacts).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# A Rosetta shell on Apple silicon also reports uname -m x86_64. Name the
# host darwin-amd64 only when sysctl shows Intel hardware. sysctl -i prints
# nothing for an OID that this macOS does not have. Any other answer stops.
check_native_intel_mac() {
    local translated arm64
    if ! command -v sysctl >/dev/null 2>&1; then
        echo "sysctl is missing, so this Mac cannot be checked for Rosetta." >&2
        exit 1
    fi
    if ! translated="$(sysctl -in sysctl.proc_translated)" \
            || ! arm64="$(sysctl -in hw.optional.arm64)"; then
        echo "sysctl could not report the Mac processor type." >&2
        exit 1
    fi
    case "${translated}/${arm64}" in
        /|0/|/0|0/0) ;;
        1/*|*/1)
            echo "This shell runs under Rosetta on an Apple silicon Mac. Open a native arm64 shell." >&2
            exit 1
            ;;
        *)
            echo "sysctl reported an unknown Mac processor type: ${translated}/${arm64}" >&2
            exit 1
            ;;
    esac
}

platform() {
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64) printf '%s\n' "linux-amd64" ;;
        Linux-aarch64|Linux-arm64) printf '%s\n' "linux-arm64" ;;
        Darwin-arm64) printf '%s\n' "darwin-arm64" ;;
        # The package key for release id x86_64-darwin. Printing the name
        # does not write a tarball. The packaging path below refuses it.
        Darwin-x86_64)
            check_native_intel_mac
            printf '%s\n' "darwin-amd64"
            ;;
        *)
            echo "Unsupported release platform: $(uname -s)-$(uname -m)" >&2
            exit 1
            ;;
    esac
}

# CI on an Intel Mac prints this name. This branch writes nothing.
if [[ "${1:-}" == --print-platform ]]; then
    platform
    exit 0
fi

PLATFORM="$(platform)"
# Refuse before staging so no darwin-amd64 tarball is written.
if [[ "$PLATFORM" == darwin-amd64 ]]; then
    echo "darwin-amd64 is not a signed release platform. See docs/MAC.md." >&2
    exit 1
fi
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

# Each staged file must be a thin executable for this package name. A
# universal binary or one built for another processor stops packaging.
# check_native_header in scripts/release-platform-input.py holds the rule.
python3 -I -S - "${ROOT}" "${PLATFORM}" "${STAGE}"/* <<'PY'
import importlib.util, json, os, sys
from pathlib import Path
root, package_key, paths = Path(sys.argv[1]), sys.argv[2], sys.argv[3:]
spec = importlib.util.spec_from_file_location("release_platform_input", root / "scripts/release-platform-input.py")
inputs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inputs)
release_id = json.loads((root / "scripts/release-platforms.json").read_text())[package_key]
refused = []
for path in map(Path, paths):
    with path.open("rb") as binary:
        header = binary.read(64)
    try:
        inputs.check_native_header(header, path.stat().st_mode, release_id, path.name)
    except ValueError as error:
        refused.append(str(error))
if refused:
    sys.exit("Refusing to package " + package_key + ":\n" + "\n".join(refused))
PY

echo "Packaging ${PACKAGE}:"
ls -l "${STAGE}"

tar -C "${STAGE_ROOT}" -czf "${ROOT}/${PACKAGE}.tar.gz" "${PACKAGE}"
(cd "${ROOT}" && shasum -a 256 "${PACKAGE}.tar.gz" > "${PACKAGE}.tar.gz.sha256")
echo "Wrote ${ROOT}/${PACKAGE}.tar.gz"
