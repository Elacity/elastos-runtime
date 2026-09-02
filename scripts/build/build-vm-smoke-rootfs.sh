#!/usr/bin/env bash
#
# Build a minimal VM smoke-test rootfs for direct crosvm validation.
# This bypasses the ElastOS runtime entirely and helps answer one question:
# can this host + crosvm + guest kernel boot a tiny guest and show userspace
# output on the serial console?
#
# Usage:
#   ./scripts/build/build-vm-smoke-rootfs.sh
#   ./scripts/build/build-vm-smoke-rootfs.sh --target aarch64 --output /tmp/elastos-vm-smoke-aarch64
#
# Output directory contains:
#   - rootfs.ext4
#   - run-vm-smoke.sh
#
# The runner is meant to be copied to the target machine alongside rootfs.ext4.

set -euo pipefail

die() { echo "Error: $*" >&2; exit 1; }
info() { echo "  ▶ $*"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

TARGET_ARCH="aarch64"
OUTPUT_DIR="/tmp/elastos-vm-smoke-aarch64"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --target)
            [[ -z "${2:-}" ]] && die "Usage: --target <aarch64|x86_64>"
            TARGET_ARCH="$2"
            shift 2
            ;;
        --output)
            [[ -z "${2:-}" ]] && die "Usage: --output <dir>"
            mkdir -p "$2"
            OUTPUT_DIR="$(cd "$2" && pwd)"
            shift 2
            ;;
        *)
            die "Unknown option: $1"
            ;;
    esac
done

case "$TARGET_ARCH" in
    aarch64)
        DEFAULT_CONSOLE="ttyS0"  # crosvm uses 16550 UART on all architectures
        CROSS_BUSYBOX="${HOME}/.local/share/elastos/cross/aarch64/bin/busybox"
        ;;
    x86_64)
        DEFAULT_CONSOLE="ttyS0"
        CROSS_BUSYBOX=""
        ;;
    *)
        die "Unsupported target arch: ${TARGET_ARCH}"
        ;;
esac

command -v mke2fs >/dev/null 2>&1 || die "mke2fs not found. Install e2fsprogs."

BUSYBOX=""
if [[ "$TARGET_ARCH" == "$(uname -m)" ]]; then
    for candidate in /bin/busybox /usr/bin/busybox "$(command -v busybox 2>/dev/null || true)"; do
        if [[ -n "$candidate" && -x "$candidate" ]]; then
            BUSYBOX="$candidate"
            break
        fi
    done
else
    [[ -x "$CROSS_BUSYBOX" ]] || die "Cross busybox not found at ${CROSS_BUSYBOX}"
    BUSYBOX="$CROSS_BUSYBOX"
fi

[[ -n "$BUSYBOX" ]] || die "busybox not found"
if command -v file >/dev/null 2>&1; then
    file "$BUSYBOX" | grep -Eq "statically linked|static-pie linked" \
        || die "busybox must be static or static-pie: ${BUSYBOX}"
fi

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
STAGING_DIR="${WORK_DIR}/staging"
ROOTFS="${OUTPUT_DIR}/rootfs.ext4"

mkdir -p "${STAGING_DIR}"/{bin,dev,etc,proc,sys,tmp}

cp "$BUSYBOX" "${STAGING_DIR}/bin/busybox"
chmod 755 "${STAGING_DIR}/bin/busybox"
for applet in sh mount cat echo sleep ls uname dmesg stty; do
    ln -sf busybox "${STAGING_DIR}/bin/${applet}"
done

cat > "${STAGING_DIR}/init" <<'EOF'
#!/bin/sh
mount -t proc proc /proc 2>/dev/null || true
mount -t sysfs sys /sys 2>/dev/null || true
mount -t devtmpfs dev /dev 2>/dev/null || true

echo "init: hello from vm smoke test"
echo "init: cmdline=$(cat /proc/cmdline)"
echo "init: uname=$(uname -a)"

for dev in /dev/console /dev/ttyAMA0 /dev/ttyS0 /dev/hvc0; do
    if [ -c "$dev" ]; then
        echo "init: detected console device $dev"
    fi
done

for dev in /dev/console /dev/ttyAMA0 /dev/ttyS0 /dev/hvc0; do
    if [ -c "$dev" ]; then
        echo "init: explicit write to $dev" >"$dev" 2>/dev/null || true
    fi
done

echo "init: sleeping 30"
sleep 30
echo "init: exiting 0"
exit 0
EOF
chmod 755 "${STAGING_DIR}/init"

mkdir -p "$OUTPUT_DIR"
mke2fs -q -t ext4 -d "${STAGING_DIR}" -F "${ROOTFS}" "64M" \
    || die "mke2fs failed"

cat > "${OUTPUT_DIR}/run-vm-smoke.sh" <<EOF
#!/usr/bin/env bash
set -euo pipefail

ROOTFS="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")" && pwd)/rootfs.ext4"
REAL_HOME="\${HOME}"
if [[ -n "\${SUDO_USER:-}" ]]; then
  SUDO_HOME="\$(getent passwd "\${SUDO_USER}" | cut -d: -f6 || true)"
  if [[ -n "\${SUDO_HOME}" ]]; then
    REAL_HOME="\${SUDO_HOME}"
  fi
fi

CROSVM_BIN="\${CROSVM_BIN:-\${REAL_HOME}/.local/share/elastos/bin/crosvm}"
VMLINUX="\${VMLINUX:-\${REAL_HOME}/.local/share/elastos/bin/vmlinux}"
CONSOLE="\${CONSOLE:-${DEFAULT_CONSOLE}}"
BOOT_ARGS="\${BOOT_ARGS:-console=\${CONSOLE} reboot=k panic=1 init=/init}"

mkdir -p /tmp/elastos/crosvm-empty

exec "\${CROSVM_BIN}" run \
  --serial type=stdout,hardware=serial,num=1,stdin \
  --block "path=\${ROOTFS},root=true,ro=true" \
  --pivot-root /tmp/elastos/crosvm-empty \
  -p "\${BOOT_ARGS}" \
  "\${VMLINUX}"
EOF
chmod 755 "${OUTPUT_DIR}/run-vm-smoke.sh"

info "Built smoke rootfs: ${ROOTFS}"
info "Runner: ${OUTPUT_DIR}/run-vm-smoke.sh"
info "Default console for ${TARGET_ARCH}: ${DEFAULT_CONSOLE}"
