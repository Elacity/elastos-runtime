#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/build/build-browser-vm-rootfs.sh --out-dir /tmp/elastos-browser-vm-rootfs [options]

Builds a bootable Browser VM rootfs artifact for development testing.

The product runtime is VM-backed. This script assembles a Debian guest
filesystem directly with debootstrap/chroot, overlays the ElastOS Browser VM
contract, then emits plain artifacts consumed by crosvm or Apple VZ:

  rootfs.ext4
  vmlinux
  initrd
  browser-vm-rootfs-manifest.json

Options:
  --out-dir PATH              Build output directory
  --target-platform PLATFORM  linux-arm64 (shared Mac/Jetson guest)
  --rootfs-size SIZE          mke2fs image size (default: 4096M)
  --debian-suite SUITE        Debian suite (default: bookworm)
  --debian-mirror URL         Debian mirror (default: https://deb.debian.org/debian)
USAGE
}

die() {
  echo "Error: $*" >&2
  exit 1
}

out_dir=""
target_platform="${ELASTOS_BROWSER_VM_TARGET_PLATFORM:-linux-arm64}"
rootfs_size="${ELASTOS_BROWSER_VM_ROOTFS_SIZE:-4096M}"
debian_suite="${ELASTOS_BROWSER_VM_DEBIAN_SUITE:-bookworm}"
debian_mirror="${ELASTOS_BROWSER_VM_DEBIAN_MIRROR:-https://deb.debian.org/debian}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out-dir)
      out_dir="${2:-}"
      shift 2
      ;;
    --target-platform)
      target_platform="${2:-}"
      shift 2
      ;;
    --rootfs-size)
      rootfs_size="${2:-}"
      shift 2
      ;;
    --debian-suite)
      debian_suite="${2:-}"
      shift 2
      ;;
    --debian-mirror)
      debian_mirror="${2:-}"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

[[ -n "$out_dir" ]] || { usage >&2; exit 2; }

image_inputs_args=(--target-platform "$target_platform" --rootfs-size "$rootfs_size"
  --debian-suite "$debian_suite" --debian-mirror "$debian_mirror" --image-dir "$out_dir")
if image_inputs=$(python3 "$repo_root/scripts/browser-vm-image-inputs.py" "${image_inputs_args[@]}"); then
  echo "[browser-vm-rootfs] reuse verified guest: recipe input hash matches" >&2
  cat "$out_dir/browser-vm-rootfs-manifest.json"
  exit 0
else
  input_status=$?
  [[ "$input_status" == 2 ]] || die "Guest input or cached image verification failed"
fi

case "$target_platform" in
  linux-arm64)
    deb_arch="arm64"
    rust_target="aarch64-unknown-linux-musl"
    kernel_package="linux-image-arm64"
    ;;
  linux-amd64)
    deb_arch="amd64"
    rust_target="x86_64-unknown-linux-musl"
    kernel_package="linux-image-amd64"
    ;;
  *)
    echo "--target-platform must be linux-arm64 or linux-amd64" >&2
    exit 2
    ;;
esac

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is required"
}

resolve_cmd() {
  local name="$1"
  local path
  path="$(command -v "$name" 2>/dev/null || true)"
  if [[ -z "$path" && -x "/usr/sbin/$name" ]]; then
    path="/usr/sbin/$name"
  fi
  if [[ -z "$path" && -x "/sbin/$name" ]]; then
    path="/sbin/$name"
  fi
  [[ -n "$path" ]] || die "$name is required"
  printf '%s\n' "$path"
}

as_root() {
  if [[ "${EUID}" -eq 0 ]]; then
    "$@"
  else
    sudo "$@"
  fi
}

require_cmd cargo
require_cmd git
mke2fs_bin="$(resolve_cmd mke2fs)"
require_cmd cpio
require_cmd gzip
require_cmd python3
require_cmd findmnt
if [[ "${EUID}" -ne 0 ]]; then
  require_cmd sudo
fi
debootstrap_bin="$(resolve_cmd debootstrap)"

mkdir -p "$out_dir"
printf '%s\n' "$image_inputs" > "$out_dir/browser-vm-inputs.json"
out_dir="$(cd "$out_dir" && pwd)"
target_dir="$out_dir/target-contract"
rootfs_dir="$out_dir/rootfs"
initrd_dir="$out_dir/initrd-root"
rootfs_image="$out_dir/rootfs.ext4"
kernel_image="$out_dir/vmlinux"
initrd_image="$out_dir/initrd"

mounted_rootfs=0
cleanup_mounts() {
  local mountpoint
  for mountpoint in \
    "$rootfs_dir/dev/pts" \
    "$rootfs_dir/dev" \
    "$rootfs_dir/proc" \
    "$rootfs_dir/sys"; do
    findmnt -R "$mountpoint" >/dev/null 2>&1 || continue
    as_root umount -R "$mountpoint" >/dev/null 2>&1 || \
      as_root umount -l "$mountpoint" >/dev/null 2>&1 || true
  done
  mounted_rootfs=0
}

require_mounts_clean() {
  local dirty=0
  local mountpoint
  for mountpoint in \
    "$rootfs_dir/dev/pts" \
    "$rootfs_dir/dev" \
    "$rootfs_dir/proc" \
    "$rootfs_dir/sys"; do
    if findmnt -R "$mountpoint" >/dev/null 2>&1; then
      echo "Error: rootfs pseudo-filesystem still mounted at $mountpoint" >&2
      dirty=1
    fi
  done
  [[ "$dirty" == "0" ]]
}
selkies_source_dir="$(mktemp -d "$out_dir/selkies-source.XXXXXX")"
trap 'cleanup_mounts; rm -rf "$selkies_source_dir"' EXIT
python3 "$repo_root/scripts/build/prepare-browser-selkies.py" --out-dir "$selkies_source_dir/source"

echo "[browser-vm-rootfs] target: $target_platform"
echo "[browser-vm-rootfs] output: $out_dir"
echo "[browser-vm-rootfs] selkies: vendored 1.6.1"

cargo_target_dir="${ELASTOS_BROWSER_VM_CARGO_TARGET_DIR:-$out_dir/cargo-target}"
echo "[browser-vm-rootfs] build guest binaries"
CARGO_TARGET_DIR="$cargo_target_dir" cargo build --quiet --locked \
  --manifest-path "$repo_root/elastos/tools/browser-native-proxy-engine/Cargo.toml" \
  --target "$rust_target" --release
CARGO_TARGET_DIR="$cargo_target_dir" cargo build --quiet --locked \
  --manifest-path "$repo_root/elastos/tools/browser-vm-runtime-relay/Cargo.toml" \
  --target "$rust_target" --release
CARGO_TARGET_DIR="$cargo_target_dir" cargo build --quiet --locked \
  --manifest-path "$repo_root/elastos/tools/browser-vm-guest-control-bridge/Cargo.toml" \
  --target "$rust_target" --release

echo "[browser-vm-rootfs] debootstrap Debian $debian_suite ($deb_arch)"
cleanup_mounts
as_root rm -rf "$rootfs_dir" "$target_dir" "$initrd_dir" "$rootfs_image" "$kernel_image" "$initrd_image" \
  "$out_dir/vmlinuz" "$out_dir/preflight.json" "$out_dir/stage-result.json" "$out_dir/kernel.version" \
  "$out_dir/browser-vm-rootfs-manifest.json" "$out_dir/node" "$out_dir/chromium"
as_root mkdir -p "$rootfs_dir"
as_root "$debootstrap_bin" \
  --arch="$deb_arch" \
  --variant=minbase \
  "$debian_suite" \
  "$rootfs_dir" \
  "$debian_mirror"

as_root mkdir -p "$rootfs_dir/opt"
as_root cp -a "$selkies_source_dir/source" "$rootfs_dir/opt/selkies-build"

as_root mount -t proc proc "$rootfs_dir/proc"
as_root mount -t sysfs sysfs "$rootfs_dir/sys"
as_root mount --bind /dev "$rootfs_dir/dev"
as_root mount --bind /dev/pts "$rootfs_dir/dev/pts"
mounted_rootfs=1
as_root cp /etc/resolv.conf "$rootfs_dir/etc/resolv.conf"

echo "[browser-vm-rootfs] disable package-time generic initramfs generation"
as_root chroot "$rootfs_dir" /bin/sh <<'SH'
set -eu
mkdir -p /usr/sbin
dpkg-divert --quiet --local --add --rename \
  --divert /usr/sbin/update-initramfs.distrib \
  /usr/sbin/update-initramfs
cat > /usr/sbin/update-initramfs <<'STUB'
#!/bin/sh
echo "elastos-browser-vm-rootfs: package-time update-initramfs skipped; builder creates controlled initrd" >&2
exit 0
STUB
chmod 755 /usr/sbin/update-initramfs
SH

echo "[browser-vm-rootfs] install Browser guest packages"
as_root chroot "$rootfs_dir" /usr/bin/env \
  DEBIAN_FRONTEND=noninteractive \
  KERNEL_PACKAGE="$kernel_package" \
  /bin/sh <<'SH'
set -eu
apt-get update -qq
apt-get install --no-install-recommends -y -qq \
  busybox-static \
  ca-certificates \
  chromium \
  dbus \
  fontconfig \
  fonts-dejavu-core \
  gcc \
  gir1.2-gst-plugins-bad-1.0 \
  gir1.2-gst-plugins-base-1.0 \
  gir1.2-gstreamer-1.0 \
  gstreamer1.0-libav \
  gstreamer1.0-nice \
  gstreamer1.0-pulseaudio \
  gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-ugly \
  gstreamer1.0-tools \
  gstreamer1.0-x \
  kmod \
  libasound2 \
  libgbm1 \
  libgtk-3-0 \
  libnss3 \
  libx11-6 \
  libxcomposite1 \
  libxdamage1 \
  libxfixes3 \
  libxkbcommon0 \
  libxrandr2 \
  libc6-dev \
  linux-libc-dev \
  nodejs \
  pipewire \
  pipewire-pulse \
  python3 \
  python3-aiohttp \
  python3-gi \
  python3-gi-cairo \
  python3-gst-1.0 \
  python3-numpy \
  python3-dev \
  python3-pip \
  python3-setuptools \
  python3-wheel \
  python3-websockets \
  tini \
  xauth \
  x11-xserver-utils \
  xclip \
  xsel \
  xvfb \
  wireplumber \
  "$KERNEL_PACKAGE"
# Preserve the former Selkies dependency set as a separate freezing step.
CC=gcc python3 -m pip install --break-system-packages --no-cache-dir -q \
  websockets basicauth gputil prometheus_client msgpack pynput psutil watchdog Pillow python-xlib
CC=gcc python3 -m pip install --break-system-packages --no-cache-dir -q \
  --no-index --no-deps --no-build-isolation /opt/selkies-build
mv /opt/selkies-build/gst-web /opt/gst-web
mkdir -p /usr/share/doc/selkies
mv /opt/selkies-build/provenance /usr/share/doc/selkies/source
rm -rf /opt/selkies-build

apt-get clean
rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*
SH

as_root chroot "$rootfs_dir" /bin/sh <<'SH'
set -eu
rm -f /usr/sbin/update-initramfs
dpkg-divert --quiet --local --rename --remove /usr/sbin/update-initramfs
SH

kernel_version="$(
  as_root chroot "$rootfs_dir" /bin/sh -lc 'for dir in /lib/modules/*; do [ -d "$dir" ] && basename "$dir"; done | sort -V | tail -n 1'
)"
[[ -n "$kernel_version" ]] || die "could not determine installed kernel version"
printf '%s\n' "$kernel_version" > "$out_dir/kernel.version"

echo "[browser-vm-rootfs] verify GStreamer Python bindings and Selkies module"
as_root chroot "$rootfs_dir" /usr/bin/env PYTHONDONTWRITEBYTECODE=1 \
  /usr/bin/python3 - < "$repo_root/scripts/build/browser-gst-python-smoke.py"

echo "[browser-vm-rootfs] verify Debian kernel/modules: $kernel_version"
as_root chroot "$rootfs_dir" /usr/bin/env KERNEL_VERSION="$kernel_version" /bin/sh <<'SH'
set -eu
test -f "/boot/vmlinuz-${KERNEL_VERSION}"
test -d "/lib/modules/${KERNEL_VERSION}"
find "/lib/modules/${KERNEL_VERSION}" -name "*vsock*.ko*" | grep -q .
test -x /bin/busybox
test -x /usr/bin/node
test -x /usr/lib/chromium/chromium
test -x /usr/bin/xrandr
test -x /usr/bin/xsel
test -x /usr/bin/pipewire
test -x /usr/bin/pipewire-pulse
test -x /usr/bin/pw-cli
test -x /usr/bin/wireplumber
test -f /opt/gst-web/index.html
python3 - <<'PY'
import importlib.util
from pathlib import Path

spec = importlib.util.find_spec("selkies_gstreamer.gstwebrtc_app")
text = Path(spec.origin).read_text()
if (
    "elastos_ice_transport_policy" not in text
    or "ice-transport-policy" not in text
    or "confirmed ICE transport policy after TURN setup" not in text
):
    raise SystemExit("Selkies must apply ElastOS relay-only ICE policy to webrtcbin")
if "_elastos_raw_caps_with_framerate" in text or "Gst.Fraction()" in text:
    raise SystemExit("Selkies must use the installed gst-python bindings without raw GI workarounds")
if "self.build_video_pipeline()\n            self.build_audio_pipeline()" in text:
    raise SystemExit("Selkies must keep video/data and audio on separate product WebRTC peers")
if "self.build_video_pipeline()" not in text or "self.build_audio_pipeline()" not in text:
    raise SystemExit("Selkies split product WebRTC peers must retain video and audio pipelines")
if "Selkies 1.6.1 audio RTP header extensions are fragile" not in text:
    raise SystemExit("Selkies split audio peer must disable fragile audio RTP extensions")
if "combined audio/video product session" in text:
    raise SystemExit("Selkies split product peers must not disable video RTP header extensions globally")
if 'pulsesrc = Gst.ElementFactory.make("pulsesrc")' not in text:
    raise SystemExit("Selkies split audio peer must use an unnamed Pulse source")
if 'opusenc = Gst.ElementFactory.make("opusenc")' not in text or "self.opusenc = opusenc" not in text:
    raise SystemExit("Selkies split audio peer must use a tracked unnamed Opus encoder")
if "Audio encoder is unavailable" not in text:
    raise SystemExit("Selkies audio bitrate update must fail clearly when the Opus encoder is unavailable")
if 'rtpopuspay_queue = Gst.ElementFactory.make("queue")' not in text:
    raise SystemExit("Selkies split audio peer must use an unnamed audio RTP queue")
if "Audio pipeline element is unavailable" not in text:
    raise SystemExit("Selkies audio pipeline must fail before linking when an audio element is unavailable")
if "forcing audio SDP offer for split product audio peer" not in text:
    raise SystemExit("Selkies split audio peer must force SDP offer negotiation")
if "Failed to add {} to pipeline" in text:
    raise SystemExit("Selkies audio pipeline must not fail on the legacy strict add return check")
if "emitting ICE candidate" not in text:
    raise SystemExit("Selkies must log outbound ICE candidates at info level")
PY
gst-inspect-1.0 webrtcbin | grep -q 'ice-transport-policy'
gst-inspect-1.0 nice >/dev/null 2>&1
gst-inspect-1.0 pulsesrc >/dev/null 2>&1
Xvfb :98 -screen 0 320x240x24 -nolisten tcp -ac >/tmp/elastos-selkies-help-xvfb.log 2>&1 &
xvfb_pid="$!"
trap 'kill "$xvfb_pid" >/dev/null 2>&1 || true' EXIT
for _ in $(seq 1 50); do
  [ -S /tmp/.X11-unix/X98 ] && break
  sleep 0.1
done
DISPLAY=:98 selkies-gstreamer --help >/dev/null 2>&1
kill "$xvfb_pid" >/dev/null 2>&1 || true
trap - EXIT
test -x /usr/local/bin/selkies-gstreamer
SH

cp "$rootfs_dir/boot/vmlinuz-${kernel_version}" "$out_dir/vmlinuz"
if gzip -t "$out_dir/vmlinuz" >/dev/null 2>&1; then
  gzip -dc "$out_dir/vmlinuz" > "$kernel_image"
else
  cp "$out_dir/vmlinuz" "$kernel_image"
fi
rm -f "$out_dir/vmlinuz"

cp "$rootfs_dir/usr/bin/node" "$out_dir/node"
cp "$rootfs_dir/usr/lib/chromium/chromium" "$out_dir/chromium"
chmod 755 "$out_dir/node" "$out_dir/chromium"

rm -rf "$target_dir"
mkdir -p "$target_dir"
"$repo_root/scripts/build/stage-browser-vm-target.sh" \
  --out-dir "$target_dir" \
  --target-platform "$target_platform" \
  --native-proxy-bin "$cargo_target_dir/$rust_target/release/browser-native-proxy-engine" \
  --runtime-relay-bin "$cargo_target_dir/$rust_target/release/browser-vm-runtime-relay" \
  --guest-control-bridge-bin "$cargo_target_dir/$rust_target/release/browser-vm-guest-control-bridge" \
  --control-service "$repo_root/scripts/browser-selkies-control-service.mjs" \
  --vz-transport-bootstrap "$repo_root/scripts/browser-vm-vz-transport-bootstrap.mjs" \
  --runtime-exit-transport vsock_relay \
  --display-backend vm_selkies_gstreamer_webrtc \
  --node-bin "$out_dir/node" \
  --chromium-bin "$out_dir/chromium" > "$out_dir/stage-result.json"

as_root cp -a "$target_dir/rootfs/." "$rootfs_dir/"
as_root chroot "$rootfs_dir" /bin/sh -lc '
set -eu
mkdir -p /var/lib/elastos/browser-profiles /run/elastos /tmp
chmod 1777 /tmp
chmod 755 /opt/elastos/bin/browser-vm-init /opt/elastos/bin/browser-vm-selkies-start
cat >/opt/elastos/bin/chromium <<'WRAPPER'
#!/bin/sh
if [ -x /usr/bin/chromium ]; then
  exec /usr/bin/chromium "\$@"
fi
if [ -x /opt/elastos/bin/chromium.real ]; then
  exec /opt/elastos/bin/chromium.real "\$@"
fi
echo "browser-vm: chromium is not installed in this guest image" >&2
exit 127
WRAPPER
chmod 755 /opt/elastos/bin/chromium
ln -sf /opt/elastos/bin/node /usr/local/bin/elastos-node
ln -sf /opt/elastos/bin/chromium /usr/local/bin/elastos-chromium
/opt/elastos/bin/node --version >/opt/elastos/browser-node.version
/usr/bin/chromium --version >/opt/elastos/browser-chromium.version
test -f /opt/gst-web/index.html
printf "%s\n" "selkies-gstreamer" >/opt/elastos/browser-selkies.entrypoint
'

echo "[browser-vm-rootfs] build tiny initrd"
rm -rf "$initrd_dir"
mkdir -p "$initrd_dir"/{bin,dev,lib/modules,newroot,proc,run,sys}
cp "$rootfs_dir/bin/busybox" "$initrd_dir/bin/busybox"
chmod 755 "$initrd_dir/bin/busybox"
for applet in sh mount mkdir cat echo sleep seq switch_root modprobe mdev grep sed cp ls tail dmesg sync chmod; do
  ln -sf busybox "$initrd_dir/bin/$applet"
done
cp "$repo_root/scripts/browser-selkies-control-service.mjs" \
  "$initrd_dir/bin/browser-selkies-control-service.mjs"
chmod 644 "$initrd_dir/bin/browser-selkies-control-service.mjs"
mkdir -p "$initrd_dir/lib/modules"
cp -a "$rootfs_dir/lib/modules/$kernel_version" "$initrd_dir/lib/modules/"
cat > "$initrd_dir/init" <<'SH'
#!/bin/sh
set -eu

export PATH=/bin:/sbin:/usr/bin:/usr/sbin

mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev 2>/dev/null || {
  mkdir -p /dev
  mdev -s 2>/dev/null || true
}
mkdir -p /newroot /run

ELASTOS_BROWSER_VM_INITRD_SERIAL_LOG_DEV=""
for candidate in /dev/hvc0 /dev/ttyS0 /dev/console; do
  if [ -w "$candidate" ]; then
    ELASTOS_BROWSER_VM_INITRD_SERIAL_LOG_DEV="$candidate"
    break
  fi
done

initrd_log() {
  printf 'browser-vm-initrd: %s\n' "$*" >&2 || true
  [ -n "$ELASTOS_BROWSER_VM_INITRD_SERIAL_LOG_DEV" ] || return 0
  printf 'browser-vm-initrd: %s\n' "$*" >"$ELASTOS_BROWSER_VM_INITRD_SERIAL_LOG_DEV" 2>/dev/null || true
}

initrd_mark_newroot() {
  [ -d /newroot/var/log/elastos ] || return 0
  printf 'browser-vm-initrd: %s\n' "$*" >>/newroot/var/log/elastos/browser-vm-initrd.log 2>/dev/null || true
}

initrd_dump_diagnostics() {
  initrd_log "cmdline: $(cat /proc/cmdline 2>/dev/null || true)"
  initrd_log "visible block devices:"
  busybox ls -l /dev/vd* /dev/sd* /dev/nvme* 2>/dev/null || true
  if [ -n "$ELASTOS_BROWSER_VM_INITRD_SERIAL_LOG_DEV" ]; then
    busybox ls -l /dev/vd* /dev/sd* /dev/nvme* >"$ELASTOS_BROWSER_VM_INITRD_SERIAL_LOG_DEV" 2>/dev/null || true
    printf 'browser-vm-initrd: mounts:\n' >"$ELASTOS_BROWSER_VM_INITRD_SERIAL_LOG_DEV" 2>/dev/null || true
    cat /proc/mounts >"$ELASTOS_BROWSER_VM_INITRD_SERIAL_LOG_DEV" 2>/dev/null || true
    printf 'browser-vm-initrd: dmesg tail:\n' >"$ELASTOS_BROWSER_VM_INITRD_SERIAL_LOG_DEV" 2>/dev/null || true
    busybox dmesg 2>/dev/null | busybox tail -n 120 >"$ELASTOS_BROWSER_VM_INITRD_SERIAL_LOG_DEV" 2>/dev/null || true
  fi
}

on_initrd_exit() {
  status=$?
  set +e
  trap - EXIT
  if [ "$status" -ne 0 ]; then
    initrd_log "exiting with status $status before rootfs handoff"
    initrd_mark_newroot "exiting with status $status before rootfs handoff"
    initrd_dump_diagnostics
  fi
  exit "$status"
}
trap on_initrd_exit EXIT

initrd_log "starting rootfs handoff"

for module in \
  crc32c_generic \
  crc32c_arm64 \
  mbcache \
  jbd2 \
  ext4 \
  virtio \
  virtio_ring \
  virtio_pci \
  virtio_rng \
  virtio_console \
  virtio_net \
  virtio_blk \
  vsock \
  vmw_vsock_virtio_transport_common \
  vmw_vsock_virtio_transport \
  virtio_vsock; do
  modprobe "$module" 2>/dev/null || true
done
initrd_log "module load pass complete"

for _ in $(seq 1 100); do
  [ -b /dev/vda ] && break
  sleep 0.1
done
[ -b /dev/vda ] || {
  initrd_log "block device /dev/vda did not appear"
  exit 1
}

if ! mount -t ext4 -o rw /dev/vda /newroot; then
  initrd_log "failed to mount /dev/vda on /newroot"
  exit 1
fi
initrd_log "mounted /dev/vda on /newroot"
mkdir -p /newroot/var/log/elastos
initrd_mark_newroot "mounted /dev/vda on /newroot"
initrd_mark_newroot "cmdline: $(cat /proc/cmdline 2>/dev/null || true)"
initrd_mark_newroot "post-mount compatibility patch start"
sync || true
if [ -f /newroot/opt/elastos/bin/browser-vm-selkies-start ] &&
  grep -q '"timeout_ms": 5000' /newroot/opt/elastos/bin/browser-vm-selkies-start; then
  sed -i 's/"timeout_ms": 5000/"timeout_ms": ${ELASTOS_BROWSER_VM_CDP_TIMEOUT_MS:-20000}/' \
    /newroot/opt/elastos/bin/browser-vm-selkies-start
fi
if [ -f /newroot/opt/elastos/bin/browser-selkies-control-service.mjs ] &&
  ! grep -q 'readBigUInt64BE' /newroot/opt/elastos/bin/browser-selkies-control-service.mjs &&
  grep -q 'Selkies WebSocket frame is too large' /newroot/opt/elastos/bin/browser-selkies-control-service.mjs; then
  sed -i 's/throw new Error("Selkies WebSocket frame is too large");/if (buffer.length < offset + 8) return null; const bigLength = buffer.readBigUInt64BE(offset); offset += 8; if (bigLength > 16777216n) { throw new Error("Selkies WebSocket frame is too large"); } length = Number(bigLength);/' \
    /newroot/opt/elastos/bin/browser-selkies-control-service.mjs
fi
if [ -f /bin/browser-selkies-control-service.mjs ]; then
  cp /bin/browser-selkies-control-service.mjs /newroot/opt/elastos/bin/browser-selkies-control-service.mjs
  chmod 644 /newroot/opt/elastos/bin/browser-selkies-control-service.mjs
fi
initrd_mark_newroot "post-mount compatibility patch complete"
if [ ! -x /newroot/opt/elastos/bin/browser-vm-init ]; then
  initrd_log "/opt/elastos/bin/browser-vm-init is missing or not executable"
  initrd_mark_newroot "/opt/elastos/bin/browser-vm-init is missing or not executable"
  exit 1
fi
if [ ! -x /newroot/bin/sh ]; then
  initrd_log "/bin/sh is missing or not executable in rootfs"
  initrd_mark_newroot "/bin/sh is missing or not executable in rootfs"
  exit 1
fi
initrd_log "exec switch_root to /opt/elastos/bin/browser-vm-init"
initrd_mark_newroot "exec switch_root to /opt/elastos/bin/browser-vm-init"
sync || true
set +e
exec switch_root /newroot /opt/elastos/bin/browser-vm-init >>/newroot/var/log/elastos/browser-vm-initrd.log 2>&1
status=$?
set -e
initrd_log "exec switch_root failed to start with status $status"
initrd_mark_newroot "exec switch_root failed to start with status $status"
sync || true
exit "$status"
SH
chmod 755 "$initrd_dir/init"
(cd "$initrd_dir" && find . -print0 | cpio --null -o --format=newc 2>/dev/null | gzip -9) > "$initrd_image"

cleanup_mounts
require_mounts_clean

echo "[browser-vm-rootfs] run rootfs preflight"
"$repo_root/scripts/browser-vm-target-preflight.sh" --target-dir "$rootfs_dir" --require-runtime-deps > "$out_dir/preflight.json"

echo "[browser-vm-rootfs] pack ext4 image"
rm -f "$rootfs_image"
as_root "$mke2fs_bin" -q -t ext4 -d "$rootfs_dir" -F "$rootfs_image" "$rootfs_size"
as_root chown "$(id -u):$(id -g)" "$rootfs_image"

current_inputs=$(python3 "$repo_root/scripts/browser-vm-image-inputs.py" "${image_inputs_args[@]:0:8}")
[[ "$current_inputs" == "$image_inputs" ]] || die "Guest inputs changed during build"

python3 - "$out_dir" "$target_platform" "$rootfs_image" "$kernel_image" "$initrd_image" <<'PY'
import hashlib
import json
import pathlib
import sys

out_dir = pathlib.Path(sys.argv[1])
target_platform = sys.argv[2]
rootfs = pathlib.Path(sys.argv[3])
kernel = pathlib.Path(sys.argv[4])
initrd = pathlib.Path(sys.argv[5])

def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()

preflight = json.loads((out_dir / "preflight.json").read_text())
manifest = {
    "schema": "elastos.browser.vm-rootfs-build/v1",
    "ok": bool(preflight.get("ok")),
    "builder": "debootstrap",
    "target_platform": target_platform,
    "inputs_sha256": json.loads((out_dir / "browser-vm-inputs.json").read_text())["sha256"],
    "recipe_options": json.loads((out_dir / "browser-vm-inputs.json").read_text())["options"],
    "rootfs_ext4": str(rootfs),
    "sha256": sha256(rootfs),
    "size": rootfs.stat().st_size,
    "kernel": {
        "path": str(kernel),
        "sha256": sha256(kernel),
        "size": kernel.stat().st_size,
        "version": (out_dir / "kernel.version").read_text().strip(),
    },
    "initrd": {
        "path": str(initrd),
        "sha256": sha256(initrd),
        "size": initrd.stat().st_size,
        "kind": "elastos-tiny-initrd",
    },
    "preflight": preflight,
}
(out_dir / "browser-vm-rootfs-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
print(json.dumps(manifest, separators=(",", ":")))
PY
