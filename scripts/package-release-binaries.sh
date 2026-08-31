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

# Top-level executables of the elastos workspace and of every own-workspace
# capsule (providers, home-cli, ...). Skip cargo bookkeeping files and
# libraries; -maxdepth 1 keeps deps/ and build/ intermediates out.
collect() {
    local dir="$1"
    [ -d "${dir}" ] || return 0
    find "${dir}" -maxdepth 1 -type f -perm -u+x \
        ! -name '*.d' ! -name '*.rlib' ! -name '*.so' ! -name '*.dylib' \
        -exec cp {} "${STAGE}/" \;
}

collect "${ROOT}/elastos/target/release"
for lock in "${ROOT}"/capsules/*/Cargo.lock; do
    collect "$(dirname "${lock}")/target/release"
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
