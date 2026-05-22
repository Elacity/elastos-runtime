#!/usr/bin/env bash
#
# scripts/dev/fetch-vz-kernel.sh — Phase 2 Day 5
#
# Downloads a known-Vz-compatible Linux kernel + initramfs +
# rootfs for `elastos vm-debug boot`. Verifies every download
# against a SHA-256 baked into this script so a CDN compromise
# can't silently swap kernels under us.
#
# Source: Ubuntu 22.04 (jammy) cloud images, arm64. The kernel
# is gzip-compressed and EFI-signed; this script decompresses
# it to a raw ARM64 Linux Image that `VZLinuxBootLoader` can
# `mmap` directly. The cloud disk image ships as qcow2; this
# script converts it to a raw image using `qemu-img` (fails
# closed with install instructions if missing).
#
# Idempotent: re-running with files already in place is a
# checksum verify only. Use `--force` to clobber, `--verify-only`
# to re-checksum without downloading.
#
# macOS-only. Does NOT touch any of the four Linux-untouched
# protected crates.

set -euo pipefail

# -----------------------------------------------------------------------------
# Pinned artifact source. The Ubuntu cloud-images team archives
# every release under a dated `release-YYYYMMDD/` path that
# never changes after publication — perfect for a baked-in
# checksum. The `latest/` symlink would defeat the security
# story, so we don't use it.
# -----------------------------------------------------------------------------
readonly UBUNTU_RELEASE="jammy"
readonly UBUNTU_REVISION="release-20260515"
readonly BASE_URL="https://cloud-images.ubuntu.com/releases/${UBUNTU_RELEASE}/${UBUNTU_REVISION}"

# Source filenames on the CDN.
readonly KERNEL_REMOTE="unpacked/ubuntu-22.04-server-cloudimg-arm64-vmlinuz-generic"
readonly INITRD_REMOTE="unpacked/ubuntu-22.04-server-cloudimg-arm64-initrd-generic"
readonly ROOTFS_REMOTE="ubuntu-22.04-server-cloudimg-arm64.img"

# SHA-256 of the *as-downloaded* bytes. Computed from Ubuntu's
# own `unpacked/SHA256SUMS` and `SHA256SUMS` files for the
# pinned release. If Ubuntu's CDN ever rewrites these artifacts
# in place (it never has for an archived release), the verify
# below will catch it.
readonly KERNEL_SHA256="b712ef9919cad88f85e25e4b924c3dacde74e866363867b7b447b7841909462a"
readonly INITRD_SHA256="8cb79fdcbf90313d7a5a315a2dc90bca7435976c3603a28929bce5feefab2b1c"
readonly ROOTFS_SHA256="0b77a1a7e708723c8e7aef4fe9fca84f0e8845c0d66b1bbd612a5b43bf916bda"

# -----------------------------------------------------------------------------
# Installation layout.
#
#   $INSTALL_DIR/cache/        verbatim downloads (compressed kernel,
#                              qcow2 rootfs) — kept around so re-runs
#                              are checksum-only
#   $INSTALL_DIR/Image         decompressed Linux Image (arm64)
#   $INSTALL_DIR/initramfs.img verbatim Ubuntu initrd
#   $INSTALL_DIR/rootfs.img    raw rootfs (qemu-img convert of qcow2)
# -----------------------------------------------------------------------------
readonly INSTALL_DIR="${HOME}/.local/share/elastos/vz-bin"
readonly CACHE_DIR="${INSTALL_DIR}/cache"

FORCE=0
VERIFY_ONLY=0

usage() {
    cat >&2 <<EOF
Usage: $(basename "$0") [--force] [--verify-only]

Fetches Ubuntu 22.04 arm64 kernel + initramfs + rootfs from
${BASE_URL}
and installs them under ${INSTALL_DIR} for use by
\`elastos vm-debug boot\`.

  --force         Re-download even if cached artifacts pass checksums.
  --verify-only   Re-checksum existing artifacts; never download.
  --help          Show this message.

Requires: curl, sha256sum (or shasum -a 256), gunzip, qemu-img.
On macOS install qemu-img with: brew install qemu
EOF
}

for arg in "$@"; do
    case "$arg" in
        --force) FORCE=1 ;;
        --verify-only) VERIFY_ONLY=1 ;;
        --help|-h) usage; exit 0 ;;
        *) echo "unknown arg: $arg" >&2; usage; exit 2 ;;
    esac
done

# Refuse to run on anything but macOS. The fetch itself would
# work anywhere, but the artifacts are arm64 Apple Silicon-only
# and `vm-debug boot` exits with a typed error on Linux —
# downloading 700 MB of garbage there is bad UX.
if [[ "$(uname)" != "Darwin" ]]; then
    echo "ERROR: this script is macOS-only. Run on Apple Silicon to use" >&2
    echo "       the artifacts with \`elastos vm-debug boot\`." >&2
    exit 1
fi

# sha256sum vs shasum -a 256 — coreutils on Mac doesn't ship
# sha256sum by default; use shasum -a 256 there.
if command -v sha256sum >/dev/null 2>&1; then
    SHA256_BIN="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
    SHA256_BIN="shasum -a 256"
else
    echo "ERROR: need sha256sum or shasum on PATH" >&2
    exit 1
fi

compute_sha256() {
    local file="$1"
    # Both sha256sum and `shasum -a 256` emit "<hash>  <file>";
    # we only want the hash.
    $SHA256_BIN "$file" | awk '{print $1}'
}

verify_sha256() {
    local file="$1"
    local expected="$2"
    local got
    got="$(compute_sha256 "$file")"
    if [[ "$got" != "$expected" ]]; then
        echo "ERROR: checksum mismatch for $file" >&2
        echo "  expected: $expected" >&2
        echo "  got:      $got" >&2
        return 1
    fi
}

# Download $1 (relative path) into $2 (local path). Atomic:
# downloads to "$2.partial" first and renames on success so a
# Ctrl-C never leaves a half-written artifact masquerading as
# done.
download() {
    local remote="$1"
    local local="$2"
    local url="${BASE_URL}/${remote}"
    echo "fetching $url"
    echo "      -> $local"
    curl -fL --progress-bar -o "${local}.partial" "$url"
    mv "${local}.partial" "$local"
}

ensure_cached() {
    local remote="$1"
    local cached="$2"
    local expected_sha256="$3"

    if [[ -f "$cached" && "$FORCE" -eq 0 ]]; then
        echo "cached: $cached"
        verify_sha256 "$cached" "$expected_sha256"
        return 0
    fi

    if [[ "$VERIFY_ONLY" -eq 1 ]]; then
        echo "ERROR: --verify-only set but $cached is missing" >&2
        echo "       remove --verify-only or run a normal fetch first" >&2
        return 1
    fi

    download "$remote" "$cached"
    verify_sha256 "$cached" "$expected_sha256"
}

mkdir -p "$CACHE_DIR"

cached_kernel="${CACHE_DIR}/$(basename "$KERNEL_REMOTE")"
cached_initrd="${CACHE_DIR}/$(basename "$INITRD_REMOTE")"
cached_rootfs="${CACHE_DIR}/$(basename "$ROOTFS_REMOTE")"

ensure_cached "$KERNEL_REMOTE" "$cached_kernel" "$KERNEL_SHA256"
ensure_cached "$INITRD_REMOTE" "$cached_initrd" "$INITRD_SHA256"
ensure_cached "$ROOTFS_REMOTE" "$cached_rootfs" "$ROOTFS_SHA256"

if [[ "$VERIFY_ONLY" -eq 1 ]]; then
    echo "verify-only: all cached artifacts match expected SHA-256."
    exit 0
fi

# -----------------------------------------------------------------------------
# Decompress kernel.
#
# Ubuntu's vmlinuz-generic is gzip-compressed; Vz wants the raw
# Linux Image. After gunzip, the file starts with `MZ` (the
# EFI/PE stub) and embeds the standard ARM64 Linux Image format
# ("ARMd" at offset 0x38, "PE\0\0" at 0x40) — `elastos-vz`'s
# kernel sanity check accepts this shape.
# -----------------------------------------------------------------------------
final_kernel="${INSTALL_DIR}/Image"
if [[ -f "$final_kernel" && "$FORCE" -eq 0 ]]; then
    echo "skip decompress: $final_kernel already exists"
else
    echo "decompressing kernel -> $final_kernel"
    gunzip -c "$cached_kernel" > "${final_kernel}.partial"
    mv "${final_kernel}.partial" "$final_kernel"
fi

# -----------------------------------------------------------------------------
# Initramfs — verbatim copy. Ubuntu's initrd is already what
# the kernel expects to find at `setInitialRamdiskURL:`.
# -----------------------------------------------------------------------------
final_initramfs="${INSTALL_DIR}/initramfs.img"
if [[ -f "$final_initramfs" && "$FORCE" -eq 0 ]]; then
    echo "skip copy: $final_initramfs already exists"
else
    echo "installing initramfs -> $final_initramfs"
    cp "$cached_initrd" "${final_initramfs}.partial"
    mv "${final_initramfs}.partial" "$final_initramfs"
fi

# -----------------------------------------------------------------------------
# Convert disk from qcow2 -> raw.
#
# Vz's `VZDiskImageStorageDeviceAttachment` reads raw bytes —
# it has no qcow2 decoder. `qemu-img convert` is the standard
# tool. We only require it for this single step; the rest of
# the script works without qemu.
# -----------------------------------------------------------------------------
final_rootfs="${INSTALL_DIR}/rootfs.img"
if [[ -f "$final_rootfs" && "$FORCE" -eq 0 ]]; then
    echo "skip convert: $final_rootfs already exists"
else
    if ! command -v qemu-img >/dev/null 2>&1; then
        echo "ERROR: qemu-img not found on PATH" >&2
        echo "       Install with: brew install qemu" >&2
        echo "       (Needed to convert Ubuntu's qcow2 cloud image to the" >&2
        echo "        raw format Vz can read.)" >&2
        exit 1
    fi
    echo "converting qcow2 -> raw: $final_rootfs"
    qemu-img convert -O raw "$cached_rootfs" "${final_rootfs}.partial"
    mv "${final_rootfs}.partial" "$final_rootfs"
fi

cat <<EOF

Done. Installed under ${INSTALL_DIR}:
  Image           $(ls -lh "$final_kernel"   | awk '{print $5}')
  initramfs.img   $(ls -lh "$final_initramfs" | awk '{print $5}')
  rootfs.img      $(ls -lh "$final_rootfs"   | awk '{print $5}')

Boot the guest with:

  cargo build -p elastos-server
  scripts/dev/sign-elastos-vz/sign.sh
  elastos vm-debug boot \\
    --rootfs    ${final_rootfs} \\
    --kernel    ${final_kernel} \\
    --initramfs ${final_initramfs} \\
    --memory-mb 1024 \\
    --boot-args 'console=hvc0 root=/dev/vda1 rw'

(Cloud-init will look for metadata; the guest kernel boot
messages will stream via tracing target \`vm_console\`. Press
Ctrl-C to stop.)
EOF
