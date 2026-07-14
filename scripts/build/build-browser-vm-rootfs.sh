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
  --target-platform PLATFORM  linux-arm64|linux-amd64 (default: linux-arm64)
  --rootfs-size SIZE          mke2fs image size (default: 8192M)
  --debian-suite SUITE        Debian suite (default: bookworm)
  --debian-mirror URL         Debian mirror (default: https://deb.debian.org/debian)
  --selkies-version VERSION   Selkies Python/web version (default: 1.6.1)
  --selkies-web-url URL       Override Selkies web tarball URL
USAGE
}

die() {
  echo "Error: $*" >&2
  exit 1
}

out_dir=""
target_platform="${ELASTOS_BROWSER_VM_TARGET_PLATFORM:-linux-arm64}"
rootfs_size="${ELASTOS_BROWSER_VM_ROOTFS_SIZE:-8192M}"
debian_suite="${ELASTOS_BROWSER_VM_DEBIAN_SUITE:-bookworm}"
debian_mirror="${ELASTOS_BROWSER_VM_DEBIAN_MIRROR:-https://deb.debian.org/debian}"
selkies_version="${ELASTOS_BROWSER_VM_SELKIES_VERSION:-1.6.1}"
selkies_web_url="${ELASTOS_BROWSER_VM_SELKIES_WEB_URL:-}"

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
    --selkies-version)
      selkies_version="${2:-}"
      shift 2
      ;;
    --selkies-web-url)
      selkies_web_url="${2:-}"
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
[[ "$selkies_version" =~ ^[0-9]+[.][0-9]+[.][0-9]+([-.][A-Za-z0-9.]+)?$ ]] || {
  echo "--selkies-version must look like 1.6.1" >&2
  exit 2
}

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
require_cmd mke2fs
require_cmd cpio
require_cmd gzip
require_cmd python3
require_cmd findmnt
if [[ "${EUID}" -ne 0 ]]; then
  require_cmd sudo
fi
debootstrap_bin="$(resolve_cmd debootstrap)"

mkdir -p "$out_dir"
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
trap cleanup_mounts EXIT

echo "[browser-vm-rootfs] target: $target_platform"
echo "[browser-vm-rootfs] output: $out_dir"
if [[ -z "$selkies_web_url" ]]; then
  selkies_web_url="https://github.com/selkies-project/selkies/releases/download/v${selkies_version}/selkies-gstreamer-web_v${selkies_version}.tar.gz"
fi
echo "[browser-vm-rootfs] selkies: $selkies_version"

cargo_target_dir="${ELASTOS_BROWSER_VM_CARGO_TARGET_DIR:-$out_dir/cargo-target}"
echo "[browser-vm-rootfs] build guest binaries"
CARGO_TARGET_DIR="$cargo_target_dir" cargo build --quiet \
  --manifest-path "$repo_root/elastos/tools/browser-native-proxy-engine/Cargo.toml" \
  --target "$rust_target" --release
CARGO_TARGET_DIR="$cargo_target_dir" cargo build --quiet \
  --manifest-path "$repo_root/elastos/tools/browser-vm-runtime-relay/Cargo.toml" \
  --target "$rust_target" --release
CARGO_TARGET_DIR="$cargo_target_dir" cargo build --quiet \
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
  SELKIES_VERSION="$selkies_version" \
  SELKIES_WEB_URL="$selkies_web_url" \
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
  python3-numpy \
  python3-dev \
  python3-pip \
  python3-websockets \
  tini \
  xauth \
  x11-xserver-utils \
  xclip \
  xsel \
  xvfb \
  wireplumber \
  "$KERNEL_PACKAGE"
CC=gcc python3 -m pip install --break-system-packages --no-cache-dir -q "selkies==${SELKIES_VERSION}"
python3 - <<'PY'
from pathlib import Path
import re

path = Path("/usr/local/lib/python3.11/dist-packages/selkies_gstreamer/gstwebrtc_app.py")
text = path.read_text()
needle = "from gi.repository import GLib, Gst, GstRtp, GstSdp, GstWebRTC\n    fract = Gst.Fraction(60, 1)"
replacement = """from gi.repository import GLib, Gst, GstRtp, GstSdp, GstWebRTC
    def _elastos_raw_caps_with_framerate(framerate):
        return Gst.caps_from_string(f"video/x-raw,framerate={int(framerate)}/1")
    fract = Gst.Fraction()"""
if "_elastos_raw_caps_with_framerate" not in text:
    if needle not in text:
        raise SystemExit("Selkies Gst.Fraction compatibility patch target not found")
    text = text.replace(needle, replacement)
initial_caps = """        # Create capabilities for ximagesrc
        self.ximagesrc_caps = Gst.caps_from_string("video/x-raw")
        self.ximagesrc_caps.set_value("framerate", Gst.Fraction(self.framerate, 1))
"""
patched_initial_caps = """        # Create capabilities for ximagesrc
        self.ximagesrc_caps = _elastos_raw_caps_with_framerate(self.framerate)
"""
if initial_caps in text:
    text = text.replace(initial_caps, patched_initial_caps)
text = text.replace(
    '        self.ximagesrc_caps.set_value("framerate", Gst.Fraction(self.framerate, 1))',
    '        self.ximagesrc_caps = _elastos_raw_caps_with_framerate(self.framerate)',
)

for set_framerate_caps in (
    """            self.ximagesrc_caps = Gst.caps_from_string("video/x-raw")
            self.ximagesrc_caps.set_value("framerate", Gst.Fraction(framerate, 1))
            self.ximagesrc_capsfilter.set_property("caps", self.ximagesrc_caps)
""",
    """            self.ximagesrc_caps = Gst.caps_from_string("video/x-raw")
            self.ximagesrc_caps.set_value("framerate", Gst.Fraction(self.framerate, 1))
            self.ximagesrc_capsfilter.set_property("caps", self.ximagesrc_caps)
""",
):
    if set_framerate_caps in text:
        text = text.replace(set_framerate_caps, """            self.ximagesrc_caps = _elastos_raw_caps_with_framerate(framerate)
            self.ximagesrc_capsfilter.set_property("caps", self.ximagesrc_caps)
""")
text = text.replace(
    '            self.ximagesrc_caps.set_value("framerate", Gst.Fraction(framerate, 1))',
    '            self.ximagesrc_caps = _elastos_raw_caps_with_framerate(framerate)',
)
text = text.replace(
    '            self.ximagesrc_caps.set_value("framerate", Gst.Fraction(self.framerate, 1))',
    '            self.ximagesrc_caps = _elastos_raw_caps_with_framerate(self.framerate)',
)
for stale_fraction in (
    "Gst.Fraction(60, 1)",
    "Gst.Fraction(self.framerate, 1)",
    "Gst.Fraction(framerate, 1)",
):
    if stale_fraction in text:
        raise SystemExit(f"stale Selkies Gst.Fraction constructor remains: {stale_fraction}")
marker = '        self.webrtcbin.set_property("latency", 0)\n'
relay_patch = '''        elastos_ice_transport_policy = os.environ.get("ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY", "").strip().lower()
        if elastos_ice_transport_policy:
            if elastos_ice_transport_policy not in ("all", "relay"):
                raise GSTWebRTCAppError("ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY must be all or relay")
            try:
                policy_value = getattr(GstWebRTC.WebRTCICETransportPolicy, elastos_ice_transport_policy.upper())
            except AttributeError:
                policy_value = elastos_ice_transport_policy
            self.webrtcbin.set_property("ice-transport-policy", policy_value)
            logger.info("using ICE transport policy: %s", elastos_ice_transport_policy)
'''
turn_marker = '''        if self.turn_servers:
            for i, turn_server in enumerate(self.turn_servers):
                logger.info("updating TURN server")
                if i == 0:
                    self.webrtcbin.set_property("turn-server", turn_server)
                else:
                    self.webrtcbin.emit("add-turn-server", turn_server)
'''
turn_relay_patch = '''        if elastos_ice_transport_policy:
            self.webrtcbin.set_property("ice-transport-policy", policy_value)
            logger.info("confirmed ICE transport policy after TURN setup: %s", elastos_ice_transport_policy)
'''
if "elastos_ice_transport_policy" not in text:
    if marker not in text:
        raise SystemExit("Selkies relay policy patch target not found")
    text = text.replace(marker, marker + relay_patch, 1)
if "confirmed ICE transport policy after TURN setup" not in text:
    if turn_marker not in text:
        raise SystemExit("Selkies TURN relay policy patch target not found")
    text = text.replace(turn_marker, turn_marker + turn_relay_patch, 1)
if "confirmed ICE transport policy after TURN setup" not in text:
    raise SystemExit("Selkies relay policy patch incomplete")
ice_log_marker = '        logger.debug("received ICE candidate: %d %s", mlineindex, candidate)\n'
ice_log_patch = '        logger.info("emitting ICE candidate: %d %s", mlineindex, candidate)\n'
if ice_log_patch not in text:
    if ice_log_marker not in text:
        raise SystemExit("Selkies ICE candidate log patch target not found")
    text = text.replace(ice_log_marker, ice_log_patch, 1)
rtp_extensions_marker = '''        rtp_id_iteration = 0
        return_result = True
'''
legacy_rtp_extensions_patch = '''        # Selkies 1.6.1 RTP header extensions are unstable in the ElastOS
        # combined audio/video product session. Runtime TURN plus NACK/FIR keeps
        # the media path reliable without mutating RTP extension caps here.
        return True
'''
if legacy_rtp_extensions_patch in text:
    text = text.replace(legacy_rtp_extensions_patch, rtp_extensions_marker, 1)
elif rtp_extensions_marker not in text:
    raise SystemExit("Selkies RTP extension patch removal target not found")
opusenc_member_marker = "        self.rtpgccbwe = None\n"
opusenc_member_patch = "        self.rtpgccbwe = None\n        self.opusenc = None\n"
if opusenc_member_patch not in text:
    if opusenc_member_marker not in text:
        raise SystemExit("Selkies opusenc member patch target not found")
    text = text.replace(opusenc_member_marker, opusenc_member_patch, 1)
video_only_start = """        if audio_only:
            self.build_audio_pipeline()
        else:
            self.build_video_pipeline()
"""
audio_video_start = """        if audio_only:
            self.build_audio_pipeline()
        else:
            self.build_video_pipeline()
            self.build_audio_pipeline()
"""
audio_video_pattern = (
    r"        if audio_only:\n"
    r"            self\.build_audio_pipeline\(\)\n"
    r"        else:\n"
    r"            self\.build_video_pipeline\(\)\n"
    r"(?:            self\.build_audio_pipeline\(\)\n)+"
)
text, pipeline_replacements = re.subn(audio_video_pattern, video_only_start, text, count=1)
if pipeline_replacements == 0 and video_only_start not in text:
    raise SystemExit("Selkies video/audio split pipeline patch target not found")
audio_extension_block = """        # Add WebRTC RTP extensions
        extensions_return = self.rtp_add_extensions(rtpopuspay, audio=True)
        if not extensions_return:
            logger.warning("WebRTC RTP extension configuration failed with audio, this may lead to suboptimal performance")
"""
legacy_audio_extension_patch = """        # Selkies 1.6.1 can corrupt combined audio/video SDP when audio RTP
        # header extensions are attached. Keep the product session audio track
        # simple and let WebRTC/NACK handle media recovery through Runtime TURN.
        extensions_return = True
"""
audio_extension_patch = """        # Selkies 1.6.1 audio RTP header extensions are fragile in the split
        # product audio peer. Keep the audio track
        # simple and let WebRTC/NACK handle media recovery through Runtime TURN.
        extensions_return = True
"""
if audio_extension_block in text:
    text = text.replace(audio_extension_block, audio_extension_patch, 1)
elif legacy_audio_extension_patch in text:
    text = text.replace(legacy_audio_extension_patch, audio_extension_patch, 1)
elif "Selkies 1.6.1 audio RTP header extensions are fragile" not in text:
    raise SystemExit("Selkies audio RTP extension patch target not found")
pulsesrc_named = '        pulsesrc = Gst.ElementFactory.make("pulsesrc", "pulsesrc")\n'
pulsesrc_unnamed = '        pulsesrc = Gst.ElementFactory.make("pulsesrc")\n'
pulsesrc_device = '        pulsesrc.set_property("device", "auto_null.monitor")\n'
if pulsesrc_named in text:
    text = text.replace(pulsesrc_named, pulsesrc_unnamed, 1)
elif pulsesrc_unnamed not in text:
    raise SystemExit("Selkies pulsesrc patch target not found")
text = re.sub(r'^[ \t]*pulsesrc\.set_property\("device", .*\)\n', '', text, flags=re.MULTILINE)
text = text.replace(pulsesrc_unnamed, pulsesrc_unnamed + pulsesrc_device, 1)
opusenc_named = '        opusenc = Gst.ElementFactory.make("opusenc", "opusenc")\n'
opusenc_unnamed = '        opusenc = Gst.ElementFactory.make("opusenc")\n'
if opusenc_named in text:
    text = text.replace(opusenc_named, opusenc_unnamed, 1)
elif opusenc_unnamed not in text:
    raise SystemExit("Selkies opusenc patch target not found")
opusenc_bitrate_marker = '        opusenc.set_property("bitrate", self.audio_bitrate)\n'
opusenc_bitrate_patch = '        opusenc.set_property("bitrate", self.audio_bitrate)\n        self.opusenc = opusenc\n'
if opusenc_bitrate_patch not in text:
    if opusenc_bitrate_marker not in text:
        raise SystemExit("Selkies opusenc reference patch target not found")
    text = text.replace(opusenc_bitrate_marker, opusenc_bitrate_patch, 1)
opusenc_update_block = """            element = Gst.Bin.get_by_name(self.pipeline, "opusenc")
            element.set_property("bitrate", bitrate)
"""
opusenc_update_patch = """            element = self.opusenc or Gst.Bin.get_by_name(self.pipeline, "opusenc")
            if element is None:
                raise GSTWebRTCAppError("Audio encoder is unavailable")
            element.set_property("bitrate", bitrate)
"""
if opusenc_update_block in text:
    text = text.replace(opusenc_update_block, opusenc_update_patch, 1)
elif opusenc_update_patch not in text:
    raise SystemExit("Selkies audio bitrate update patch target not found")
audio_queue_named = '        rtpopuspay_queue = Gst.ElementFactory.make("queue", "rtpopuspay_queue")\n'
audio_queue_unnamed = '        rtpopuspay_queue = Gst.ElementFactory.make("queue")\n'
if audio_queue_named in text:
    text = text.replace(audio_queue_named, audio_queue_unnamed, 1)
elif audio_queue_unnamed not in text:
    raise SystemExit("Selkies audio queue patch target not found")
audio_add_block = """        # Add all elements to the pipeline.
        pipeline_elements = [pulsesrc, pulsesrc_capsfilter, opusenc, rtpopuspay, rtpopuspay_queue, rtpopuspay_capsfilter]

        for pipeline_element in pipeline_elements:
            self.pipeline.add(pipeline_element)
"""
audio_add_strict_block = """        # Add all elements to the pipeline.
        pipeline_elements = [pulsesrc, pulsesrc_capsfilter, opusenc, rtpopuspay, rtpopuspay_queue, rtpopuspay_capsfilter]

        for pipeline_element in pipeline_elements:
            if pipeline_element is None:
                raise GSTWebRTCAppError("Audio pipeline element is unavailable")
            if not self.pipeline.add(pipeline_element):
                raise GSTWebRTCAppError("Failed to add {} to pipeline".format(pipeline_element.get_name()))
"""
audio_add_patch = """        # Add all elements to the pipeline.
        pipeline_elements = [pulsesrc, pulsesrc_capsfilter, opusenc, rtpopuspay, rtpopuspay_queue, rtpopuspay_capsfilter]

        for pipeline_element in pipeline_elements:
            if pipeline_element is None:
                raise GSTWebRTCAppError("Audio pipeline element is unavailable")
            self.pipeline.add(pipeline_element)
"""
if audio_add_strict_block in text:
    text = text.replace(audio_add_strict_block, audio_add_patch, 1)
elif audio_add_block in text:
    text = text.replace(audio_add_block, audio_add_patch, 1)
elif audio_add_patch not in text:
    raise SystemExit("Selkies audio pipeline add patch target not found")
audio_offer_marker = '        logger.info("{} pipeline started".format("audio" if audio_only else "video"))\n'
audio_offer_patch = """        logger.info("{} pipeline started".format("audio" if audio_only else "video"))
        if audio_only:
            logger.info("forcing audio SDP offer for split product audio peer")
            self.__on_negotiation_needed(self.webrtcbin)
"""
if audio_offer_patch not in text:
    if audio_offer_marker not in text:
        raise SystemExit("Selkies split audio offer patch target not found")
    text = text.replace(audio_offer_marker, audio_offer_patch, 1)
path.write_text(text)
PY
python3 - <<'PY'
import os
import pathlib
import shutil
import tarfile
import tempfile
import urllib.request

url = os.environ["SELKIES_WEB_URL"]
install_parent = pathlib.Path("/opt")
install_dir = install_parent / "gst-web"
with tempfile.NamedTemporaryFile(suffix=".tar.gz") as archive:
    urllib.request.urlretrieve(url, archive.name)
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        root = tmp_path.resolve()
        with tarfile.open(archive.name, "r:gz") as tar:
            for member in tar.getmembers():
                target = (tmp_path / member.name).resolve()
                if target != root and root not in target.parents:
                    raise SystemExit(f"unsafe path in Selkies web archive: {member.name}")
            tar.extractall(tmp_path)
        extracted = tmp_path / "gst-web"
        if not (extracted / "index.html").is_file():
            raise SystemExit("Selkies web archive must contain gst-web/index.html")
        if install_dir.exists():
            shutil.rmtree(install_dir)
        shutil.move(str(extracted), str(install_dir))
if not (install_dir / "index.html").is_file():
    raise SystemExit("Selkies web install missing /opt/gst-web/index.html")
PY
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
python3 -c "import selkies_gstreamer" >/dev/null 2>&1
python3 - <<'PY'
import gi
for namespace in ("Gst", "GstWebRTC", "GstSdp", "GstRtp"):
    gi.require_version(namespace, "1.0")
from gi.repository import Gst, GstWebRTC, GstSdp, GstRtp
Gst.init(None)
PY
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
if "_elastos_raw_caps_with_framerate" not in text:
    raise SystemExit("Selkies must avoid Gst.Fraction(framerate, 1) on this PyGObject build")
for stale_fraction in (
    "Gst.Fraction(60, 1)",
    "Gst.Fraction(self.framerate, 1)",
    "Gst.Fraction(framerate, 1)",
):
    if stale_fraction in text:
        raise SystemExit(f"Selkies stale Gst.Fraction constructor remains: {stale_fraction}")
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
as_root mke2fs -q -t ext4 -d "$rootfs_dir" -F "$rootfs_image" "$rootfs_size"
as_root chown "$(id -u):$(id -g)" "$rootfs_image"

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
