#!/usr/bin/env bash
set -euo pipefail

die() {
  echo "Error: $*" >&2
  exit 1
}

usage() {
  cat <<'USAGE'
Usage:
  scripts/build/stage-browser-vm-target.sh --out-dir /tmp/browser-vm-target [options]

Stages the Browser VM guest contract into <out-dir>/rootfs and runs the static
Browser VM target preflight. This does not install packages, launch crosvm, or
launch Apple VZ.

Options:
  --native-proxy-bin PATH       browser-native-proxy-engine binary
  --runtime-relay-bin PATH      browser-vm-runtime-relay binary
  --guest-control-bridge-bin PATH
                                browser-vm-guest-control-bridge binary
  --control-service PATH        browser-selkies-control-service.mjs
  --node-bin PATH               guest node binary
  --chromium-bin PATH           guest Chromium binary
  --target-platform PLATFORM    linux-amd64|linux-arm64 (default: host Linux arch,
                                linux-arm64 on macOS)
  --runtime-exit-transport X    vsock_relay|carrier_stream (default: vsock_relay)
  --display-backend X           vm_selkies_gstreamer_webrtc|vm_native_webrtc
                                (default: vm_selkies_gstreamer_webrtc)
  --rootfs-ext4 PATH            Also pack staged rootfs into an ext4 image
  --rootfs-size SIZE            mke2fs size when --rootfs-ext4 is used (default: 2048M)
USAGE
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
out_dir=""
native_proxy_bin="${ELASTOS_BROWSER_NATIVE_PROXY_BIN:-}"
runtime_relay_bin="${ELASTOS_BROWSER_VM_RUNTIME_RELAY_BIN:-}"
guest_control_bridge_bin="${ELASTOS_BROWSER_VM_GUEST_CONTROL_BRIDGE_BIN:-}"
control_service="${ELASTOS_BROWSER_SELKIES_CONTROL_SERVICE:-${repo_root}/scripts/browser-selkies-control-service.mjs}"
node_bin="${ELASTOS_BROWSER_VM_NODE_BIN:-}"
chromium_bin="${ELASTOS_BROWSER_VM_CHROMIUM_BIN:-}"
target_platform="${ELASTOS_BROWSER_VM_TARGET_PLATFORM:-}"
runtime_exit_transport="${ELASTOS_BROWSER_VM_RUNTIME_EXIT_TRANSPORT:-vsock_relay}"
display_backend="${ELASTOS_BROWSER_VM_DISPLAY_BACKEND:-vm_selkies_gstreamer_webrtc}"
rootfs_ext4=""
rootfs_size="2048M"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out-dir)
      out_dir="${2:-}"
      shift 2
      ;;
    --native-proxy-bin)
      native_proxy_bin="${2:-}"
      shift 2
      ;;
    --runtime-relay-bin)
      runtime_relay_bin="${2:-}"
      shift 2
      ;;
    --guest-control-bridge-bin)
      guest_control_bridge_bin="${2:-}"
      shift 2
      ;;
    --control-service)
      control_service="${2:-}"
      shift 2
      ;;
    --node-bin)
      node_bin="${2:-}"
      shift 2
      ;;
    --chromium-bin)
      chromium_bin="${2:-}"
      shift 2
      ;;
    --target-platform)
      target_platform="${2:-}"
      shift 2
      ;;
    --runtime-exit-transport)
      runtime_exit_transport="${2:-}"
      shift 2
      ;;
    --display-backend)
      display_backend="${2:-}"
      shift 2
      ;;
    --rootfs-ext4)
      rootfs_ext4="${2:-}"
      shift 2
      ;;
    --rootfs-size)
      rootfs_size="${2:-}"
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
if [[ -z "$target_platform" ]]; then
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) target_platform="linux-amd64" ;;
    Linux-aarch64|Linux-arm64|Darwin-*) target_platform="linux-arm64" ;;
    *) die "cannot infer target platform; pass --target-platform linux-amd64|linux-arm64" ;;
  esac
fi
case "$target_platform" in
  linux-amd64|linux-arm64) ;;
  *) die "--target-platform must be linux-amd64 or linux-arm64" ;;
esac
case "$runtime_exit_transport" in
  carrier_stream|vsock_relay) ;;
  *) die "--runtime-exit-transport must be carrier_stream or vsock_relay" ;;
esac
case "$display_backend" in
  vm_selkies_gstreamer_webrtc|vm_native_webrtc) ;;
  *) die "--display-backend must be vm_selkies_gstreamer_webrtc or vm_native_webrtc" ;;
esac

first_executable() {
  local candidate
  for candidate in "$@"; do
    if [[ -n "$candidate" && -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

resolve_required_file() {
  local label="$1"
  local path="$2"
  [[ -n "$path" ]] || die "$label is required"
  [[ -f "$path" ]] || die "$label does not exist: $path"
  printf '%s\n' "$path"
}

resolve_required_executable() {
  local label="$1"
  local path="$2"
  [[ -n "$path" ]] || die "$label is required"
  [[ -x "$path" ]] || die "$label is not executable: $path"
  printf '%s\n' "$path"
}

validate_linux_guest_binary() {
  local label="$1"
  local path="$2"
  python3 - "$label" "$path" "$target_platform" <<'PY'
import pathlib
import struct
import sys

label, path, target_platform = sys.argv[1:]
data = pathlib.Path(path).read_bytes()[:20]
if len(data) < 20 or data[:4] != b"\x7fELF":
    raise SystemExit(f"{label} must be a Linux ELF guest binary for {target_platform}")
if data[5] == 1:
    endian = "<"
elif data[5] == 2:
    endian = ">"
else:
    raise SystemExit(f"{label} has an invalid ELF data encoding")
machine = struct.unpack(endian + "H", data[18:20])[0]
expected = {
    "linux-amd64": 62,
    "linux-arm64": 183,
}[target_platform]
if machine != expected:
    names = {62: "x86_64", 183: "aarch64"}
    got = names.get(machine, f"e_machine={machine}")
    want = names[expected]
    raise SystemExit(f"{label} must be {want} for {target_platform}, got {got}")
PY
}

if [[ -z "$native_proxy_bin" ]]; then
  native_proxy_bin="$(first_executable \
    "${repo_root}/elastos/tools/browser-native-proxy-engine/target/release/browser-native-proxy-engine" \
    "${repo_root}/elastos/tools/browser-native-proxy-engine/target/debug/browser-native-proxy-engine" \
    || true)"
fi
if [[ -z "$runtime_relay_bin" ]]; then
  runtime_relay_bin="$(first_executable \
    "${repo_root}/elastos/tools/browser-vm-runtime-relay/target/release/browser-vm-runtime-relay" \
    "${repo_root}/elastos/tools/browser-vm-runtime-relay/target/debug/browser-vm-runtime-relay" \
    || true)"
fi
if [[ -z "$guest_control_bridge_bin" ]]; then
  guest_control_bridge_bin="$(first_executable \
    "${repo_root}/elastos/tools/browser-vm-guest-control-bridge/target/release/browser-vm-guest-control-bridge" \
    "${repo_root}/elastos/tools/browser-vm-guest-control-bridge/target/debug/browser-vm-guest-control-bridge" \
    || true)"
fi
if [[ -z "$node_bin" ]]; then
  if [[ "$(uname -s)" != "Darwin" ]]; then
    node_bin="$(command -v node 2>/dev/null || true)"
  fi
fi
if [[ -z "$chromium_bin" ]]; then
  if [[ "$(uname -s)" != "Darwin" ]]; then
    chromium_bin="$(first_executable \
      /usr/bin/chromium \
      /usr/bin/chromium-browser \
      /usr/bin/google-chrome \
      || true)"
  fi
fi

native_proxy_bin="$(resolve_required_executable "browser-native-proxy-engine" "$native_proxy_bin")"
runtime_relay_bin="$(resolve_required_executable "browser-vm-runtime-relay" "$runtime_relay_bin")"
guest_control_bridge_bin="$(resolve_required_executable "browser-vm-guest-control-bridge" "$guest_control_bridge_bin")"
control_service="$(resolve_required_file "browser-selkies-control-service.mjs" "$control_service")"
node_bin="$(resolve_required_executable "node" "$node_bin")"
chromium_bin="$(resolve_required_executable "chromium" "$chromium_bin")"

validate_linux_guest_binary "browser-native-proxy-engine" "$native_proxy_bin"
validate_linux_guest_binary "browser-vm-runtime-relay" "$runtime_relay_bin"
validate_linux_guest_binary "browser-vm-guest-control-bridge" "$guest_control_bridge_bin"
validate_linux_guest_binary "node" "$node_bin"
validate_linux_guest_binary "chromium" "$chromium_bin"

staging_dir="${out_dir%/}/rootfs"
rm -rf "$staging_dir"
mkdir -p \
  "$staging_dir/etc/elastos" \
  "$staging_dir/opt/elastos/bin" \
  "$staging_dir/usr/bin" \
  "$staging_dir/run/elastos" \
  "$staging_dir/tmp"
chmod 1777 "$staging_dir/tmp"

install -m 755 "$native_proxy_bin" "$staging_dir/opt/elastos/bin/browser-native-proxy-engine"
install -m 755 "$runtime_relay_bin" "$staging_dir/opt/elastos/bin/browser-vm-runtime-relay"
install -m 755 "$guest_control_bridge_bin" "$staging_dir/opt/elastos/bin/browser-vm-guest-control-bridge"
install -m 644 "$control_service" "$staging_dir/opt/elastos/bin/browser-selkies-control-service.mjs"
install -m 755 "$node_bin" "$staging_dir/opt/elastos/bin/node"
install -m 755 "$chromium_bin" "$staging_dir/opt/elastos/bin/chromium.real"
cat > "$staging_dir/opt/elastos/bin/chromium" <<'SH'
#!/bin/sh
if [ -x /usr/bin/chromium ]; then
  exec /usr/bin/chromium "$@"
fi
if [ -x /opt/elastos/bin/chromium.real ]; then
  exec /opt/elastos/bin/chromium.real "$@"
fi
echo "browser-vm: chromium is not installed in this guest image" >&2
exit 127
SH
chmod 755 "$staging_dir/opt/elastos/bin/chromium"

cat > "$staging_dir/etc/elastos/browser-vm-target.json" <<JSON
{
  "schema": "elastos.browser.vm-target/v1",
  "engine": "chromium_microvm",
  "network_mode": "runtime_net_only",
  "direct_network": false,
  "wallet_injection": false,
  "media_transport": "runtime_relay",
  "display_mode": "webrtc_remote_display",
  "guarantee_level": "mechanism_microvm",
  "display_backend": "${display_backend}",
  "runtime_exit_transport": "${runtime_exit_transport}",
  "control_transport": "vsock_relay",
  "control_port": 19092
}
JSON

cat > "$staging_dir/opt/elastos/bin/browser-vm-init" <<'SH'
#!/bin/sh
set -eu

export PATH="/opt/elastos/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

mkdir -p /dev /proc /sys /run /dev/shm /tmp /var/log/elastos
mount -t devtmpfs devtmpfs /dev || true
mount -t proc proc /proc 2>/dev/null || true
mount -t sysfs sysfs /sys 2>/dev/null || true
mount -t tmpfs tmpfs /run -o mode=0755,nosuid,nodev 2>/dev/null || true
mount -t tmpfs shm /dev/shm -o mode=1777,nosuid,nodev 2>/dev/null || true
mount -t tmpfs tmpfs /tmp -o mode=1777,nosuid,nodev 2>/dev/null || true

rootfs_mark() {
  printf 'browser-vm-init: %s\n' "$*" >>/var/log/elastos/browser-vm-rootfs-entry.log 2>/dev/null || true
}

rootfs_checkpoint() {
  rootfs_mark "$*"
  sync 2>/dev/null || true
}

ELASTOS_BROWSER_VM_SERIAL_LOG_DEV=""
export ELASTOS_BROWSER_VM_SERIAL_LOG_DEV

rootfs_mark "entered rootfs init"
rootfs_mark "cmdline: $(cat /proc/cmdline 2>/dev/null || true)"

sync 2>/dev/null || true
ELASTOS_BROWSER_VM_LOG_FILE=/var/log/elastos/browser-vm-init.log
export ELASTOS_BROWSER_VM_LOG_FILE
ELASTOS_BROWSER_VM_LOG_DIR=/var/log/elastos
export ELASTOS_BROWSER_VM_LOG_DIR
rootfs_mark "opening main init log: $ELASTOS_BROWSER_VM_LOG_FILE"
: > "$ELASTOS_BROWSER_VM_LOG_FILE"
rootfs_mark "opened main init log"
exec >>"$ELASTOS_BROWSER_VM_LOG_FILE" 2>&1
echo "browser-vm-init: logging to $ELASTOS_BROWSER_VM_LOG_FILE"
rootfs_checkpoint "main init log redirected"

rootfs_checkpoint "rootfs diagnostics initialized"

dump_browser_logs() {
  for log in /var/log/elastos/browser-vm-*.log; do
    [ -f "$log" ] || continue
    echo "===== $log =====" >&2
    tail -n 120 "$log" >&2 || true
  done
}

on_exit() {
  status=$?
  set +e
  trap - EXIT
  rootfs_mark "exiting with status $status"
  sync 2>/dev/null || true
  if [ "$status" -ne 0 ]; then
    dump_browser_logs
  fi
  if [ "$status" -ne 0 ] && [ -n "${ELASTOS_BROWSER_VM_CONTROL_BRIDGE_PID:-}" ]; then
    kill "$ELASTOS_BROWSER_VM_CONTROL_BRIDGE_PID" 2>/dev/null || true
  fi
  exit "$status"
}
trap on_exit EXIT
rootfs_checkpoint "exit trap installed"

cmdline_value() {
  key="$1"
  for param in $(cat /proc/cmdline 2>/dev/null || true); do
    case "$param" in
      "$key="*)
        printf '%s\n' "${param#"$key="}"
        return 0
        ;;
    esac
  done
  return 1
}

mount_if_needed() {
  fs="$1"
  src="$2"
  dest="$3"
  opts="${4:-}"
  mkdir -p "$dest"
  if grep -qs " $dest " /proc/mounts 2>/dev/null; then
    return 0
  fi
  if [ -n "$opts" ]; then
    mount -t "$fs" -o "$opts" "$src" "$dest" 2>/dev/null || true
  else
    mount -t "$fs" "$src" "$dest" 2>/dev/null || true
  fi
}

mount_if_needed proc proc /proc
mount_if_needed sysfs sysfs /sys
mount_if_needed devtmpfs devtmpfs /dev
mount_if_needed tmpfs tmpfs /run "mode=0755,nosuid,nodev"
mount_if_needed tmpfs shm /dev/shm "mode=1777,nosuid,nodev"
mount_if_needed tmpfs tmpfs /tmp "mode=1777,nosuid,nodev"
rootfs_checkpoint "runtime filesystems mounted"

for module in virtio virtio_ring virtio_pci virtio_console virtio_net; do
  modprobe "$module" 2>/dev/null || true
done

mkdir -p /run/elastos /tmp
chmod 1777 /tmp
rootfs_checkpoint "boot modules loaded"

set_guest_clock_from_cmdline() {
  epoch="$(cmdline_value elastos.browser_epoch || true)"
  case "$epoch" in
    ""|*[!0-9]*)
      return 0
      ;;
  esac
  if [ "$epoch" -lt 1700000000 ]; then
    echo "browser-vm-init: ignoring stale host epoch $epoch" >&2
    return 0
  fi
  if date -u -s "@$epoch" >/dev/null 2>&1; then
    echo "browser-vm-init: guest clock set from host epoch $epoch" >&2
  else
    echo "browser-vm-init: failed to set guest clock from host epoch $epoch" >&2
  fi
}

set_guest_clock_from_cmdline
rootfs_checkpoint "guest clock initialized"

IP_CMD_AVAILABLE=0
if command -v ip >/dev/null 2>&1; then
  ip_cmd() { ip "$@"; }
  IP_CMD_AVAILABLE=1
elif command -v busybox >/dev/null 2>&1; then
  ip_cmd() { busybox ip "$@"; }
  IP_CMD_AVAILABLE=1
else
  ip_cmd() { return 127; }
fi

if [ "$IP_CMD_AVAILABLE" = "1" ]; then
  ip_cmd link set lo up 2>/dev/null || true
elif command -v busybox >/dev/null 2>&1; then
  busybox ifconfig lo 127.0.0.1 up 2>/dev/null || true
fi

ELASTOS_BROWSER_VM_TRANSPORT="$(cmdline_value elastos.browser_transport || printf '%s\n' "${ELASTOS_BROWSER_VM_TRANSPORT:-vsock}")"
ELASTOS_BROWSER_VM_HOST_IP="$(cmdline_value elastos.browser_host_ip || printf '%s\n' "${ELASTOS_BROWSER_VM_HOST_IP:-}")"
ELASTOS_BROWSER_VM_GUEST_IP="$(cmdline_value elastos.browser_guest_ip || printf '%s\n' "${ELASTOS_BROWSER_VM_GUEST_IP:-}")"
ELASTOS_BROWSER_VM_NET_PREFIX="$(cmdline_value elastos.browser_net_prefix || printf '%s\n' "${ELASTOS_BROWSER_VM_NET_PREFIX:-30}")"

if [ "$ELASTOS_BROWSER_VM_TRANSPORT" = "private_tcp" ]; then
  for module in virtio_pci virtio_net; do
    modprobe "$module" 2>/dev/null || true
  done
  [ -n "$ELASTOS_BROWSER_VM_HOST_IP" ] || {
    echo "browser-vm-init: private_tcp requires elastos.browser_host_ip" >&2
    exit 1
  }
  [ -n "$ELASTOS_BROWSER_VM_GUEST_IP" ] || {
    echo "browser-vm-init: private_tcp requires elastos.browser_guest_ip" >&2
    exit 1
  }
  if [ "$IP_CMD_AVAILABLE" != "1" ]; then
    echo "browser-vm-init: private_tcp requires iproute2 or busybox ip in the guest image" >&2
    exit 127
  fi
  ELASTOS_BROWSER_VM_NET_IFACE=""
  for attempt in 1 2 3 4 5 6 7 8 9 10; do
    for candidate in /sys/class/net/*; do
      candidate="$(basename "$candidate")"
      [ "$candidate" = "lo" ] && continue
      ELASTOS_BROWSER_VM_NET_IFACE="$candidate"
      break
    done
    [ -n "$ELASTOS_BROWSER_VM_NET_IFACE" ] && break
    sleep 0.1
  done
  [ -n "$ELASTOS_BROWSER_VM_NET_IFACE" ] || {
    echo "browser-vm-init: private_tcp requires a guest network interface" >&2
    exit 1
  }
  ip_cmd link set "$ELASTOS_BROWSER_VM_NET_IFACE" up
  ip_cmd addr add "$ELASTOS_BROWSER_VM_GUEST_IP/$ELASTOS_BROWSER_VM_NET_PREFIX" dev "$ELASTOS_BROWSER_VM_NET_IFACE" 2>/dev/null || true
  ip_cmd route del default 2>/dev/null || true
  echo "browser-vm-init: private TCP transport on $ELASTOS_BROWSER_VM_NET_IFACE guest=$ELASTOS_BROWSER_VM_GUEST_IP host=$ELASTOS_BROWSER_VM_HOST_IP" >&2
fi
rootfs_checkpoint "transport network initialized"

for module in vsock vmw_vsock_virtio_transport vmw_vsock_virtio_transport_common virtio_console; do
  modprobe "$module" 2>/dev/null || true
done
rootfs_checkpoint "vsock modules loaded"

ELASTOS_BROWSER_VM_RELAY_PORT="$(cmdline_value elastos.browser_relay_port || printf '%s\n' "${ELASTOS_BROWSER_VM_RELAY_PORT:-19091}")"
ELASTOS_BROWSER_VM_CONTROL_BRIDGE_PORT="$(cmdline_value elastos.browser_control_port || printf '%s\n' "${ELASTOS_BROWSER_VM_CONTROL_BRIDGE_PORT:-19092}")"
: "${ELASTOS_BROWSER_VM_GUEST_RELAY_IPC:=/run/elastos/browser-exit.sock}"
: "${ELASTOS_BROWSER_VM_CONTROL_SOCKET:=/run/elastos/browser-selkies-control.sock}"
ELASTOS_BROWSER_VM_CONTROL_BRIDGE_PID=""

if [ "$ELASTOS_BROWSER_VM_TRANSPORT" = "private_tcp" ]; then
  cat > /run/elastos/browser-vm-runtime-relay.json <<JSON
{
  "schema": "elastos.browser.vm-runtime-relay.config/v1",
  "guest_relay_ipc_path": "${ELASTOS_BROWSER_VM_GUEST_RELAY_IPC}",
  "network_mode": "runtime_net_only",
  "direct_network": false,
  "transport": {
    "kind": "tcp_connect",
    "host": "${ELASTOS_BROWSER_VM_HOST_IP}",
    "port": ${ELASTOS_BROWSER_VM_RELAY_PORT}
  },
  "replace_existing_socket": true
}
JSON
else
  cat > /run/elastos/browser-vm-runtime-relay.json <<JSON
{
  "schema": "elastos.browser.vm-runtime-relay.config/v1",
  "guest_relay_ipc_path": "${ELASTOS_BROWSER_VM_GUEST_RELAY_IPC}",
  "network_mode": "runtime_net_only",
  "direct_network": false,
  "transport": {
    "kind": "vsock_listen",
    "port": ${ELASTOS_BROWSER_VM_RELAY_PORT}
  },
  "replace_existing_socket": true
}
JSON
fi
rootfs_checkpoint "runtime relay config written"

ELASTOS_BROWSER_VM_RUNTIME_RELAY_CONFIG="$(cat /run/elastos/browser-vm-runtime-relay.json)" \
  /opt/elastos/bin/browser-vm-runtime-relay &

echo "browser-vm-init: runtime relay started at ${ELASTOS_BROWSER_VM_GUEST_RELAY_IPC}" >&2
rootfs_checkpoint "runtime relay started"

rootfs_checkpoint "starting browser stack"
if ! /opt/elastos/bin/browser-vm-selkies-start; then
  echo "browser-vm-init: Browser stack failed to start" >&2
  dump_browser_logs
  exit 1
fi
rootfs_checkpoint "browser stack started"

(
  trap '' HUP
  export ELASTOS_BROWSER_VM_SERIAL_LOG_DEV
  ELASTOS_BROWSER_SELKIES_CONTROL_CONFIG="$(cat /run/elastos/browser-selkies-control.json)" \
    exec /opt/elastos/bin/node /opt/elastos/bin/browser-selkies-control-service.mjs
) >"$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-selkies-control.log" 2>&1 &
echo "$!" >/run/elastos/browser-selkies-control.pid

for _ in $(seq 1 100); do
  [ -S "$ELASTOS_BROWSER_VM_CONTROL_SOCKET" ] && break
  sleep 0.1
done
[ -S "$ELASTOS_BROWSER_VM_CONTROL_SOCKET" ] || {
  echo "browser-vm-init: Browser control socket did not start" >&2
  dump_browser_logs
  exit 1
}
rootfs_checkpoint "browser control socket present"

if [ "$ELASTOS_BROWSER_VM_TRANSPORT" = "private_tcp" ]; then
  cat > /run/elastos/browser-vm-control-bridge.json <<JSON
{
  "schema": "elastos.browser.vm-guest-control-bridge.config/v1",
  "guest_control_socket_path": "${ELASTOS_BROWSER_VM_CONTROL_SOCKET}",
  "network_mode": "runtime_net_only",
  "direct_network": false,
  "control_socket_ready_timeout_ms": 60000,
  "control_request_timeout_ms": 120000,
  "transport": {
    "kind": "tcp_listen",
    "host": "${ELASTOS_BROWSER_VM_GUEST_IP}",
    "port": ${ELASTOS_BROWSER_VM_CONTROL_BRIDGE_PORT}
  }
}
JSON
else
  cat > /run/elastos/browser-vm-control-bridge.json <<JSON
{
  "schema": "elastos.browser.vm-guest-control-bridge.config/v1",
  "guest_control_socket_path": "${ELASTOS_BROWSER_VM_CONTROL_SOCKET}",
  "network_mode": "runtime_net_only",
  "direct_network": false,
  "control_socket_ready_timeout_ms": 60000,
  "control_request_timeout_ms": 120000,
  "transport": {
    "kind": "vsock_listen",
    "port": ${ELASTOS_BROWSER_VM_CONTROL_BRIDGE_PORT}
  }
}
JSON
fi

ELASTOS_BROWSER_VM_CONTROL_BRIDGE_CONFIG="$(cat /run/elastos/browser-vm-control-bridge.json)" \
  /opt/elastos/bin/browser-vm-guest-control-bridge &
ELASTOS_BROWSER_VM_CONTROL_BRIDGE_PID="$!"
echo "$ELASTOS_BROWSER_VM_CONTROL_BRIDGE_PID" >/run/elastos/browser-vm-guest-control-bridge.pid
echo "browser-vm-init: Browser control socket ready; guest control bridge listening via ${ELASTOS_BROWSER_VM_TRANSPORT} on port ${ELASTOS_BROWSER_VM_CONTROL_BRIDGE_PORT}" >&2
rootfs_checkpoint "guest control bridge started"
echo "browser-vm-init: waiting on guest control bridge pid ${ELASTOS_BROWSER_VM_CONTROL_BRIDGE_PID}" >&2
wait "$ELASTOS_BROWSER_VM_CONTROL_BRIDGE_PID"
SH
chmod 755 "$staging_dir/opt/elastos/bin/browser-vm-init"

cat > "$staging_dir/opt/elastos/bin/browser-vm-selkies-start" <<'SH'
#!/bin/sh
set -eu

: "${ELASTOS_BROWSER_VM_LOG_FILE:=/var/log/elastos/browser-vm-init.log}"
mkdir -p "$(dirname "$ELASTOS_BROWSER_VM_LOG_FILE")"
exec >>"$ELASTOS_BROWSER_VM_LOG_FILE" 2>&1
: "${ELASTOS_BROWSER_VM_LOG_DIR:=/var/log/elastos}"
mkdir -p "$ELASTOS_BROWSER_VM_LOG_DIR"

selkies_checkpoint() {
  printf 'browser-vm-selkies-start: %s\n' "$*" >>"$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-rootfs-entry.log" 2>/dev/null || true
  echo "browser-vm-selkies-start: $*" >&2
  sync 2>/dev/null || true
}

dump_browser_logs() {
  for log in "$ELASTOS_BROWSER_VM_LOG_DIR"/browser-vm-*.log; do
    [ -f "$log" ] || continue
    echo "===== $log =====" >&2
    tail -n 120 "$log" >&2 || true
  done
}

on_selkies_exit() {
  status=$?
  set +e
  trap - EXIT
  if [ "$status" -ne 0 ]; then
    selkies_checkpoint "failed with exit $status"
    dump_browser_logs
  fi
  exit "$status"
}
trap on_selkies_exit EXIT
selkies_checkpoint "startup entered"

mkdir -p /run/elastos /var/lib/elastos/browser-profiles /tmp

: "${DISPLAY:=:40}"
: "${ELASTOS_BROWSER_VM_WIDTH:=1280}"
: "${ELASTOS_BROWSER_VM_HEIGHT:=720}"
: "${ELASTOS_BROWSER_VM_CDP_PORT:=9222}"
: "${ELASTOS_BROWSER_VM_SELKIES_PORT:=8080}"
: "${ELASTOS_BROWSER_VM_CDP_TIMEOUT_MS:=15000}"
: "${ELASTOS_BROWSER_VM_GUEST_RELAY_IPC:=/run/elastos/browser-exit.sock}"
: "${ELASTOS_BROWSER_VM_CONTROL_SOCKET:=/run/elastos/browser-selkies-control.sock}"
: "${ELASTOS_BROWSER_VM_PROFILE_DIR:=/var/lib/elastos/browser-profile}"
: "${ELASTOS_BROWSER_VM_NETWORK_MODE:=runtime_net_only}"
: "${ELASTOS_BROWSER_VM_SELKIES_AUTH_USER:=elastos}"
: "${ELASTOS_BROWSER_VM_SELKIES_AUTH_PASSWORD:=local-vm}"
: "${ELASTOS_BROWSER_VM_SELKIES_ENCODER:=openh264enc}"
: "${ELASTOS_BROWSER_VM_SELKIES_FRAMERATE:=30}"
: "${ELASTOS_BROWSER_VM_SELKIES_VIDEO_BITRATE:=16000}"
: "${ELASTOS_BROWSER_VM_SELKIES_AUDIO_BITRATE:=128000}"
: "${ELASTOS_BROWSER_VM_SELKIES_AUDIO_CHANNELS:=2}"
: "${XDG_RUNTIME_DIR:=/run/elastos/browser-runtime}"
: "${PIPEWIRE_RUNTIME_DIR:=$XDG_RUNTIME_DIR}"
: "${PULSE_RUNTIME_PATH:=$XDG_RUNTIME_DIR/pulse}"
: "${PULSE_SERVER:=unix:$PULSE_RUNTIME_PATH/native}"
: "${XDG_CONFIG_HOME:=$XDG_RUNTIME_DIR/config}"
export DISPLAY
export XDG_RUNTIME_DIR PIPEWIRE_RUNTIME_DIR PULSE_RUNTIME_PATH PULSE_SERVER XDG_CONFIG_HOME
export GIO_USE_VFS=local
selkies_checkpoint "environment initialized"

cmdline_value() {
  key="$1"
  for arg in $(cat /proc/cmdline 2>/dev/null || true); do
    case "$arg" in
      "$key="*)
        printf '%s\n' "${arg#"$key="}"
        return 0
        ;;
    esac
  done
  return 1
}

profile_key="$(cmdline_value elastos.browser_profile || true)"
profile_disk_policy="$(cmdline_value elastos.browser_profile_disk || true)"
mount_browser_profile_disk() {
  key="$1"
  disk="/dev/vdb"
  mount_dir="/var/lib/elastos/browser-profile-disk"
  for _ in $(seq 1 100); do
    [ -b "$disk" ] && break
    sleep 0.1
  done
  if [ ! -b "$disk" ]; then
    echo "browser-vm-selkies-start: principal-owned Browser profile disk is required but $disk is missing" >&2
    exit 1
  fi
  mkdir -p "$mount_dir"
  if ! grep -qs " $mount_dir " /proc/mounts 2>/dev/null; then
    if ! mount -t ext4 -o rw,noatime "$disk" "$mount_dir" 2>/dev/null; then
      command -v mke2fs >/dev/null 2>&1 || {
        echo "browser-vm-selkies-start: mke2fs is required to initialize Browser profile disk" >&2
        exit 1
      }
      mke2fs -q -t ext4 -F "$disk"
      mount -t ext4 -o rw,noatime "$disk" "$mount_dir"
    fi
  fi
  mkdir -p "$mount_dir/profiles/$key"
  chmod 700 "$mount_dir" "$mount_dir/profiles" "$mount_dir/profiles/$key" 2>/dev/null || true
  ELASTOS_BROWSER_VM_PROFILE_DIR="$mount_dir/profiles/$key"
}
case "$profile_key" in
  ""|*[!A-Za-z0-9._-]*)
    if [ "$profile_disk_policy" = "required" ]; then
      echo "browser-vm-selkies-start: principal-owned Browser profile disk requires a safe profile key" >&2
      exit 1
    fi
    ;;
  *)
    if [ "$profile_disk_policy" = "required" ]; then
      mount_browser_profile_disk "$profile_key"
    else
      ELASTOS_BROWSER_VM_PROFILE_DIR="/var/lib/elastos/browser-profiles/$profile_key"
    fi
    ;;
esac
selkies_checkpoint "profile initialized"

display_width="$(cmdline_value elastos.browser_width || true)"
display_height="$(cmdline_value elastos.browser_height || true)"
validate_display_dimension() {
  value="$1"
  min="$2"
  max="$3"
  label="$4"
  case "$value" in
    ""|*[!0-9]*)
      echo "browser-vm-selkies-start: ${label} must be an integer" >&2
      exit 1
      ;;
  esac
  if [ "$value" -lt "$min" ] || [ "$value" -gt "$max" ]; then
    echo "browser-vm-selkies-start: ${label} must be ${min}..${max}" >&2
    exit 1
  fi
  printf '%s\n' "$value"
}
if [ -n "$display_width" ] || [ -n "$display_height" ]; then
  ELASTOS_BROWSER_VM_WIDTH="$(validate_display_dimension "$display_width" 320 3840 elastos.browser_width)"
  ELASTOS_BROWSER_VM_HEIGHT="$(validate_display_dimension "$display_height" 240 2160 elastos.browser_height)"
fi

if [ "$ELASTOS_BROWSER_VM_NETWORK_MODE" != "runtime_net_only" ]; then
  echo "browser-vm-selkies-start: only runtime_net_only is supported" >&2
  exit 1
fi

case "$ELASTOS_BROWSER_VM_SELKIES_ENCODER" in
  nvh264enc|nvh265enc|nvav1enc|vah264enc|vah265enc|vavp9enc|vaav1enc|x264enc|openh264enc|x265enc|vp8enc|vp9enc|svtav1enc|av1enc|rav1enc) ;;
  *)
    echo "browser-vm-selkies-start: ELASTOS_BROWSER_VM_SELKIES_ENCODER must be a Selkies GStreamer encoder" >&2
    exit 1
    ;;
esac

ELASTOS_BROWSER_VM_SELKIES_FRAMERATE="$(validate_display_dimension "$ELASTOS_BROWSER_VM_SELKIES_FRAMERATE" 1 120 ELASTOS_BROWSER_VM_SELKIES_FRAMERATE)"
ELASTOS_BROWSER_VM_SELKIES_VIDEO_BITRATE="$(validate_display_dimension "$ELASTOS_BROWSER_VM_SELKIES_VIDEO_BITRATE" 1 100000 ELASTOS_BROWSER_VM_SELKIES_VIDEO_BITRATE)"
ELASTOS_BROWSER_VM_SELKIES_AUDIO_BITRATE="$(validate_display_dimension "$ELASTOS_BROWSER_VM_SELKIES_AUDIO_BITRATE" 16000 512000 ELASTOS_BROWSER_VM_SELKIES_AUDIO_BITRATE)"
ELASTOS_BROWSER_VM_SELKIES_AUDIO_CHANNELS="$(validate_display_dimension "$ELASTOS_BROWSER_VM_SELKIES_AUDIO_CHANNELS" 1 8 ELASTOS_BROWSER_VM_SELKIES_AUDIO_CHANNELS)"
selkies_checkpoint "display config initialized"

command -v Xvfb >/dev/null 2>&1 || {
  echo "browser-vm-selkies-start: Xvfb is required in the Browser VM image" >&2
  exit 127
}
command -v python3 >/dev/null 2>&1 || {
  echo "browser-vm-selkies-start: python3 is required in the Browser VM image" >&2
  exit 127
}
command -v pipewire >/dev/null 2>&1 || {
  echo "browser-vm-selkies-start: PipeWire is required for Browser audio" >&2
  exit 127
}
command -v pipewire-pulse >/dev/null 2>&1 || {
  echo "browser-vm-selkies-start: pipewire-pulse is required for Browser audio" >&2
  exit 127
}
command -v wireplumber >/dev/null 2>&1 || {
  echo "browser-vm-selkies-start: WirePlumber is required for Browser audio" >&2
  exit 127
}
command -v pw-cli >/dev/null 2>&1 || {
  echo "browser-vm-selkies-start: pw-cli is required for Browser audio" >&2
  exit 127
}
if command -v gst-inspect-1.0 >/dev/null 2>&1 &&
  ! gst-inspect-1.0 pulsesrc >/dev/null 2>&1; then
  echo "browser-vm-selkies-start: GStreamer pulsesrc is required for Browser audio" >&2
  exit 127
fi
selkies_checkpoint "dependencies checked"

configure_browser_wireplumber_headless() {
  mkdir -p \
    "$XDG_CONFIG_HOME/wireplumber/main.lua.d" \
    "$XDG_CONFIG_HOME/wireplumber/bluetooth.lua.d" \
    "$XDG_CONFIG_HOME/wireplumber/wireplumber.conf.d"
  cat >"$XDG_CONFIG_HOME/wireplumber/main.lua.d/80-elastos-headless.lua" <<'LUA'
if alsa_monitor and alsa_monitor.properties then
  alsa_monitor.properties["alsa.reserve"] = false
end
LUA
  cat >"$XDG_CONFIG_HOME/wireplumber/bluetooth.lua.d/80-elastos-headless.lua" <<'LUA'
if bluez_monitor and bluez_monitor.properties then
  bluez_monitor.properties["with-logind"] = false
end
if bluez_monitor then
  bluez_monitor.enabled = false
end
LUA
  cat >"$XDG_CONFIG_HOME/wireplumber/wireplumber.conf.d/80-elastos-headless.conf" <<'CONF'
wireplumber.profiles = {
  main = {
    support.dbus = disabled
    support.logind = disabled
    support.reserve-device = disabled
    monitor.alsa.reserve-device = disabled
    monitor.bluez = disabled
    monitor.bluez.seat-monitoring = disabled
  }
}
CONF
  {
    echo "XDG_CONFIG_HOME=$XDG_CONFIG_HOME"
    find "$XDG_CONFIG_HOME/wireplumber" -type f -maxdepth 3 -print | sort
  } >"$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-wireplumber-config.log" 2>&1 || true
}

start_browser_audio_stack() {
  mkdir -p "$XDG_RUNTIME_DIR" "$PULSE_RUNTIME_PATH"
  chmod 700 "$XDG_RUNTIME_DIR" 2>/dev/null || true
  configure_browser_wireplumber_headless

  export ELASTOS_BROWSER_VM_PIPEWIRE_LOG="$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-pipewire.log"
  export ELASTOS_BROWSER_VM_WIREPLUMBER_LOG="$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-wireplumber.log"
  export ELASTOS_BROWSER_VM_PIPEWIRE_PULSE_LOG="$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-pipewire-pulse.log"
  export ELASTOS_BROWSER_VM_PIPEWIRE_DUMP_LOG="$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-pipewire-dump.log"
  export ELASTOS_BROWSER_VM_PIPEWIRE_SUMMARY_LOG="$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-pipewire-summary.log"
  dbus-run-session -- sh -c '
    set -u
    pipewire >"$ELASTOS_BROWSER_VM_PIPEWIRE_LOG" 2>&1 &
    pipewire_pid=$!
    for _ in $(seq 1 120); do
      pw-cli info 0 >/dev/null 2>&1 && break
      kill -0 "$pipewire_pid" 2>/dev/null || exit 1
      sleep 0.1
    done
    pw-cli info 0 >/dev/null 2>&1 || exit 1

    wireplumber >"$ELASTOS_BROWSER_VM_WIREPLUMBER_LOG" 2>&1 &
    wireplumber_pid=$!
    for _ in $(seq 1 120); do
      pw-cli ls Client 2>/dev/null | grep -q "WirePlumber" && break
      kill -0 "$wireplumber_pid" 2>/dev/null || exit 1
      sleep 0.1
    done
    pw-cli ls Client 2>/dev/null | grep -q "WirePlumber" || exit 1

    pipewire-pulse >"$ELASTOS_BROWSER_VM_PIPEWIRE_PULSE_LOG" 2>&1 &
    wait
  ' >"$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-audio-session.log" 2>&1 &

  for _ in $(seq 1 120); do
    [ -S "$PULSE_RUNTIME_PATH/native" ] && break
    sleep 0.1
  done
  if [ ! -S "$PULSE_RUNTIME_PATH/native" ]; then
    echo "browser-vm-selkies-start: Pulse-compatible Browser audio socket did not start" >&2
    exit 1
  fi

  : >"$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-pipewire-null-sink.log"
  for _ in $(seq 1 80); do
    if pw-cli ls Node 2>/dev/null | grep -q 'auto_null'; then
      return 0
    fi
    {
      echo "waiting for pipewire-pulse module-always-sink auto_null"
      pw-cli ls Node 2>&1 || true
    } >"$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-pipewire-null-sink.log"
    sleep 0.25
  done
  echo "browser-vm-selkies-start: Browser Pulse monitor source did not become available" >&2
  exit 1
}

dump_browser_audio_graph() {
  {
    echo "=== browser audio environment ==="
    printf 'XDG_RUNTIME_DIR=%s\n' "$XDG_RUNTIME_DIR"
    printf 'PIPEWIRE_RUNTIME_DIR=%s\n' "$PIPEWIRE_RUNTIME_DIR"
    printf 'PULSE_RUNTIME_PATH=%s\n' "$PULSE_RUNTIME_PATH"
    printf 'PULSE_SERVER=%s\n' "$PULSE_SERVER"
    echo "=== browser audio sockets ==="
    ls -la "$XDG_RUNTIME_DIR" "$PULSE_RUNTIME_PATH" 2>&1 || true
    echo "=== pw-cli info 0 ==="
    pw-cli info 0 2>&1 || true
    echo "=== pw-cli ls Node ==="
    pw-cli ls Node 2>&1 || true
    echo "=== pw-cli ls Client ==="
    pw-cli ls Client 2>&1 || true
    echo "=== pw-cli ls Port ==="
    pw-cli ls Port 2>&1 || true
    echo "=== pw-cli ls Link ==="
    pw-cli ls Link 2>&1 || true
    echo "=== pw-link outputs ==="
    if command -v pw-link >/dev/null 2>&1; then
      pw-link -o 2>&1 || true
    else
      echo "pw-link unavailable"
    fi
    echo "=== pw-link inputs ==="
    if command -v pw-link >/dev/null 2>&1; then
      pw-link -i 2>&1 || true
    else
      echo "pw-link unavailable"
    fi
    echo "=== pw-link links ==="
    if command -v pw-link >/dev/null 2>&1; then
      pw-link -l 2>&1 || true
    else
      echo "pw-link unavailable"
    fi
    echo "=== pw-dump compact audio facts ==="
    if command -v pw-dump >/dev/null 2>&1; then
      pw-dump 2>/dev/null | grep -E '"(type|id|node.name|node.description|media.class|application.name|client.api|object.path|factory.name|pulse.server.type|audio.position)"' || true
    else
      echo "pw-dump unavailable"
    fi
  } >"$ELASTOS_BROWSER_VM_PIPEWIRE_SUMMARY_LOG" 2>&1 || true

  {
    echo "=== pw-cli info 0 ==="
    pw-cli info 0 2>&1 || true
    echo "=== pw-cli ls Node ==="
    pw-cli ls Node 2>&1 || true
    echo "=== pw-cli ls Port ==="
    pw-cli ls Port 2>&1 || true
    echo "=== pw-cli ls Link ==="
    pw-cli ls Link 2>&1 || true
    echo "=== pw-dump ==="
    if command -v pw-dump >/dev/null 2>&1; then
      pw-dump 2>&1 || true
    else
      echo "pw-dump unavailable"
    fi
  } >"$ELASTOS_BROWSER_VM_PIPEWIRE_DUMP_LOG" 2>&1 || true
}

mkdir -p "$ELASTOS_BROWSER_VM_PROFILE_DIR"
mkdir -p /tmp/.X11-unix
chmod 1777 /tmp/.X11-unix
rm -f /tmp/.X11-unix/X${DISPLAY#:} /tmp/.X${DISPLAY#:}-lock "$ELASTOS_BROWSER_VM_CONTROL_SOCKET"

selkies_checkpoint "starting xvfb"
Xvfb "$DISPLAY" -screen 0 "${ELASTOS_BROWSER_VM_WIDTH}x${ELASTOS_BROWSER_VM_HEIGHT}x24" \
  +extension COMPOSITE +extension DAMAGE +extension GLX +extension RANDR \
  +extension RENDER +extension MIT-SHM +extension XFIXES +extension XTEST \
  -nolisten tcp -ac -noreset >"$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-xvfb.log" 2>&1 &

for _ in $(seq 1 100); do
  [ -S "/tmp/.X11-unix/X${DISPLAY#:}" ] && break
  sleep 0.1
done
[ -S "/tmp/.X11-unix/X${DISPLAY#:}" ] || {
  echo "browser-vm-selkies-start: Xvfb did not create display socket" >&2
  exit 1
}
selkies_checkpoint "xvfb ready"
selkies_checkpoint "starting audio"
start_browser_audio_stack
dump_browser_audio_graph
selkies_checkpoint "audio ready"

selkies_checkpoint "starting native proxy"
ELASTOS_BROWSER_NATIVE_PROXY_ENGINE_CONFIG="$(cat <<JSON
{
  "schema": "elastos.browser.native-proxy-engine.config/v1",
  "browser_program": "/opt/elastos/bin/chromium",
  "relay_ipc_path": "${ELASTOS_BROWSER_VM_GUEST_RELAY_IPC}",
  "startup_grace_ms": 1000,
  "browser_args": [
    "--proxy-server={proxy_url}",
    "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1",
    "--disable-dev-shm-usage",
    "--no-first-run",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-default-apps",
    "--disable-infobars",
    "--disable-sync",
    "--disable-client-side-phishing-detection",
    "--disable-domain-reliability",
    "--disable-features=AutofillServerCommunication,OptimizationHints,OptimizationGuideModelDownloading,OptimizationTargetPrediction,MediaRouter,DialMediaRouteProvider,GlobalMediaControls,Translate",
    "--safebrowsing-disable-auto-update",
    "--disable-quic",
    "--disable-gpu",
    "--no-sandbox",
    "--autoplay-policy=no-user-gesture-required",
    "--kiosk",
    "--start-fullscreen",
    "--window-position=0,0",
    "--window-size=${ELASTOS_BROWSER_VM_WIDTH},${ELASTOS_BROWSER_VM_HEIGHT}",
    "--app=about:blank",
    "--user-data-dir=${ELASTOS_BROWSER_VM_PROFILE_DIR}",
    "--remote-debugging-address=127.0.0.1",
    "--remote-debugging-port=${ELASTOS_BROWSER_VM_CDP_PORT}"
  ]
}
JSON
)" \
ELASTOS_BROWSER_ENGINE_URL="about:blank" \
ELASTOS_BROWSER_ENGINE_STREAM_ID="stream:browser-vm-init" \
  /opt/elastos/bin/browser-native-proxy-engine >"$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-native-proxy.log" 2>"$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-chromium.log" &

ELASTOS_BROWSER_NATIVE_PROXY_URL="$(
  /opt/elastos/bin/node - "$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-native-proxy.log" <<'NODE'
const fs = require("node:fs");
const logPath = process.argv[2];
const deadline = Date.now() + 30000;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
function readyProxyUrl() {
  let text = "";
  try {
    text = fs.readFileSync(logPath, "utf8");
  } catch {
    return "";
  }
  for (const line of text.trimEnd().split(/\r?\n/).reverse()) {
    if (!line.trim()) continue;
    try {
      const event = JSON.parse(line);
      const proxyUrl = String(event.proxy_url || "");
      if (
        event.schema === "elastos.browser.native-proxy-engine.ready/v1" &&
        /^http:\/\/(?:127\.0\.0\.1|localhost):[0-9]+\/?$/.test(proxyUrl)
      ) {
        return proxyUrl.replace(/\/$/, "");
      }
    } catch {}
  }
  return "";
}
(async () => {
  while (Date.now() < deadline) {
    const proxyUrl = readyProxyUrl();
    if (proxyUrl) {
      process.stdout.write(proxyUrl);
      return;
    }
    await sleep(100);
  }
  throw new Error(`timed out waiting for native proxy ready event in ${logPath}`);
})().catch((error) => {
  console.error(error && error.message ? error.message : String(error));
  process.exit(1);
});
NODE
)"
export ELASTOS_BROWSER_NATIVE_PROXY_URL
selkies_checkpoint "native proxy ready"

if command -v selkies-gstreamer >/dev/null 2>&1; then
  set -- selkies-gstreamer
elif python3 -c 'import selkies_gstreamer' >/dev/null 2>&1; then
  set -- python3 -m selkies_gstreamer
elif python3 -c 'import selkies' >/dev/null 2>&1; then
  set -- python3 -m selkies
else
  echo "browser-vm-selkies-start: Selkies GStreamer is required in the Browser VM image" >&2
  exit 127
fi
selkies_checkpoint "selkies command resolved"

: "${ELASTOS_BROWSER_VM_ICE_SERVER:=}"
: "${ELASTOS_BROWSER_VM_ICE_SERVERS_JSON:=}"
: "${ELASTOS_BROWSER_VM_ICE_USERNAME:=}"
: "${ELASTOS_BROWSER_VM_ICE_CREDENTIAL:=}"
: "${ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY:=relay}"
: "${ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4:=}"
: "${ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4:=}"
: "${ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX:=}"

patch_selkies_relay_policy() {
  python3 - <<'PY'
import importlib.util
import re
from pathlib import Path

spec = importlib.util.find_spec("selkies_gstreamer.gstwebrtc_app")
if not spec or not spec.origin:
    raise SystemExit("browser-vm-selkies-start: Selkies gstwebrtc_app.py not found")

path = Path(spec.origin)
text = path.read_text()
fraction_needle = "from gi.repository import GLib, Gst, GstRtp, GstSdp, GstWebRTC\n    fract = Gst.Fraction(60, 1)"
fraction_replacement = """from gi.repository import GLib, Gst, GstRtp, GstSdp, GstWebRTC
    def _elastos_raw_caps_with_framerate(framerate):
        return Gst.caps_from_string(f"video/x-raw,framerate={int(framerate)}/1")
    fract = Gst.Fraction()"""
if "_elastos_raw_caps_with_framerate" not in text:
    if fraction_needle not in text:
        raise SystemExit("browser-vm-selkies-start: Selkies Gst.Fraction compatibility patch target not found")
    text = text.replace(fraction_needle, fraction_replacement, 1)
initial_caps = """        # Create capabilities for ximagesrc
        self.ximagesrc_caps = Gst.caps_from_string("video/x-raw")
        self.ximagesrc_caps.set_value("framerate", Gst.Fraction(self.framerate, 1))
"""
patched_initial_caps = """        # Create capabilities for ximagesrc
        self.ximagesrc_caps = _elastos_raw_caps_with_framerate(self.framerate)
"""
if initial_caps in text:
    text = text.replace(initial_caps, patched_initial_caps, 1)
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
""", 1)
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
        raise SystemExit(f"browser-vm-selkies-start: stale Selkies Gst.Fraction constructor remains: {stale_fraction}")
marker = '        self.webrtcbin.set_property("latency", 0)\n'
patch = '''        elastos_ice_transport_policy = os.environ.get("ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY", "").strip().lower()
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
turn_patch = '''        if elastos_ice_transport_policy:
            self.webrtcbin.set_property("ice-transport-policy", policy_value)
            logger.info("confirmed ICE transport policy after TURN setup: %s", elastos_ice_transport_policy)
'''
if "elastos_ice_transport_policy" not in text:
    if marker not in text:
        raise SystemExit("browser-vm-selkies-start: Selkies relay patch target not found")
    text = text.replace(marker, marker + patch, 1)
if "confirmed ICE transport policy after TURN setup" not in text:
    if turn_marker not in text:
        raise SystemExit("browser-vm-selkies-start: Selkies TURN patch target not found")
    text = text.replace(turn_marker, turn_marker + turn_patch, 1)
if "confirmed ICE transport policy after TURN setup" not in text:
    raise SystemExit("browser-vm-selkies-start: Selkies relay policy patch incomplete")
ice_log_marker = '        logger.debug("received ICE candidate: %d %s", mlineindex, candidate)\n'
ice_log_patch = '        logger.info("emitting ICE candidate: %d %s", mlineindex, candidate)\n'
if ice_log_patch not in text:
    if ice_log_marker not in text:
        raise SystemExit("browser-vm-selkies-start: Selkies ICE candidate log patch target not found")
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
    raise SystemExit("browser-vm-selkies-start: Selkies RTP extension patch removal target not found")
opusenc_member_marker = "        self.rtpgccbwe = None\n"
opusenc_member_patch = "        self.rtpgccbwe = None\n        self.opusenc = None\n"
if opusenc_member_patch not in text:
    if opusenc_member_marker not in text:
        raise SystemExit("browser-vm-selkies-start: Selkies opusenc member patch target not found")
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
    raise SystemExit("browser-vm-selkies-start: Selkies video/audio split pipeline patch target not found")
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
    raise SystemExit("browser-vm-selkies-start: Selkies audio RTP extension patch target not found")
pulsesrc_named = '        pulsesrc = Gst.ElementFactory.make("pulsesrc", "pulsesrc")\n'
pulsesrc_unnamed = '        pulsesrc = Gst.ElementFactory.make("pulsesrc")\n'
pulsesrc_device = '        pulsesrc.set_property("device", "auto_null.monitor")\n'
if pulsesrc_named in text:
    text = text.replace(pulsesrc_named, pulsesrc_unnamed, 1)
elif pulsesrc_unnamed not in text:
    raise SystemExit("browser-vm-selkies-start: Selkies pulsesrc patch target not found")
text = re.sub(r'^[ \t]*pulsesrc\.set_property\("device", .*\)\n', '', text, flags=re.MULTILINE)
text = text.replace(pulsesrc_unnamed, pulsesrc_unnamed + pulsesrc_device, 1)
opusenc_named = '        opusenc = Gst.ElementFactory.make("opusenc", "opusenc")\n'
opusenc_unnamed = '        opusenc = Gst.ElementFactory.make("opusenc")\n'
if opusenc_named in text:
    text = text.replace(opusenc_named, opusenc_unnamed, 1)
elif opusenc_unnamed not in text:
    raise SystemExit("browser-vm-selkies-start: Selkies opusenc patch target not found")
opusenc_bitrate_marker = '        opusenc.set_property("bitrate", self.audio_bitrate)\n'
opusenc_bitrate_patch = '        opusenc.set_property("bitrate", self.audio_bitrate)\n        self.opusenc = opusenc\n'
if opusenc_bitrate_patch not in text:
    if opusenc_bitrate_marker not in text:
        raise SystemExit("browser-vm-selkies-start: Selkies opusenc reference patch target not found")
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
    raise SystemExit("browser-vm-selkies-start: Selkies audio bitrate update patch target not found")
audio_queue_named = '        rtpopuspay_queue = Gst.ElementFactory.make("queue", "rtpopuspay_queue")\n'
audio_queue_unnamed = '        rtpopuspay_queue = Gst.ElementFactory.make("queue")\n'
if audio_queue_named in text:
    text = text.replace(audio_queue_named, audio_queue_unnamed, 1)
elif audio_queue_unnamed not in text:
    raise SystemExit("browser-vm-selkies-start: Selkies audio queue patch target not found")
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
    raise SystemExit("browser-vm-selkies-start: Selkies audio pipeline add patch target not found")
audio_offer_marker = '        logger.info("{} pipeline started".format("audio" if audio_only else "video"))\n'
audio_offer_patch = """        logger.info("{} pipeline started".format("audio" if audio_only else "video"))
        if audio_only:
            logger.info("forcing audio SDP offer for split product audio peer")
            self.__on_negotiation_needed(self.webrtcbin)
"""
if audio_offer_patch not in text:
    if audio_offer_marker not in text:
        raise SystemExit("browser-vm-selkies-start: Selkies split audio offer patch target not found")
    text = text.replace(audio_offer_marker, audio_offer_patch, 1)
path.write_text(text)
PY
}

setup_media_relay_network() {
  for module in virtio virtio_ring virtio_pci virtio_net; do
    modprobe "$module" 2>/dev/null || true
  done
  found_media_iface=""
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    for iface_path in /sys/class/net/*; do
      [ -e "$iface_path" ] || continue
      iface="${iface_path##*/}"
      [ "$iface" != "lo" ] || continue
      found_media_iface="$iface"
      break
    done
    [ -n "$found_media_iface" ] && break
    sleep 0.1
  done
  for iface_path in /sys/class/net/*; do
    [ -e "$iface_path" ] || continue
    iface="${iface_path##*/}"
    [ "$iface" != "lo" ] || continue
    if command -v ip >/dev/null 2>&1; then
      ip link set "$iface" up 2>/dev/null || true
    elif command -v busybox >/dev/null 2>&1; then
      busybox ip link set "$iface" up 2>/dev/null || busybox ifconfig "$iface" up 2>/dev/null || true
    fi
    if command -v dhclient >/dev/null 2>&1; then
      dhclient -1 "$iface" >/dev/null 2>&1 || true
    elif command -v busybox >/dev/null 2>&1; then
      busybox udhcpc -q -n -i "$iface" >/dev/null 2>&1 || true
    fi
    configure_static_media_relay_ipv4 "$iface"
    echo "browser-vm-selkies-start: media relay network configured on ${iface}" >&2
    if command -v ip >/dev/null 2>&1; then
      ip addr show dev "$iface" >&2 || true
      ip route >&2 || true
      ip -6 route >&2 || true
    elif command -v busybox >/dev/null 2>&1; then
      busybox ip addr show dev "$iface" >&2 || true
      busybox ip route >&2 || true
      busybox ip -6 route >&2 || true
    fi
    return 0
  done
  echo "browser-vm-selkies-start: no non-loopback media relay network interface found" >&2
}

has_media_relay_ipv4() {
  iface="$1"
  if command -v ip >/dev/null 2>&1; then
    ip -4 addr show dev "$iface" 2>/dev/null | grep -q ' inet '
  elif command -v busybox >/dev/null 2>&1; then
    busybox ip -4 addr show dev "$iface" 2>/dev/null | grep -q ' inet '
  else
    return 1
  fi
}

configure_static_media_relay_ipv4() {
  iface="$1"
  [ -f /run/elastos/browser-media-relay-network.env ] || return 0
  # shellcheck disable=SC1091
  . /run/elastos/browser-media-relay-network.env
  [ -n "${ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4:-}" ] || return 0
  [ -n "${ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX:-}" ] || return 0
  if has_media_relay_ipv4 "$iface"; then
    return 0
  fi
  if command -v ip >/dev/null 2>&1; then
    ip addr add "${ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4}/${ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX}" dev "$iface" 2>/dev/null || true
  elif command -v busybox >/dev/null 2>&1; then
    busybox ip addr add "${ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4}/${ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX}" dev "$iface" 2>/dev/null || true
  fi
  echo "browser-vm-selkies-start: media relay IPv4 ${ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4}/${ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX} assigned on ${iface}" >&2
}

log_media_relay_config() {
  echo "browser-vm-selkies-start: ICE config follows" >&2
  cat /run/elastos/browser-rtc.json >&2 || true
  echo "browser-vm-selkies-start: media relay network config follows" >&2
  cat /run/elastos/browser-media-relay-network.json >&2 || true
}

/opt/elastos/bin/node <<'NODE'
const fs = require("node:fs");

function fail(message) {
  console.error(`browser-vm-selkies-start: ${message}`);
  process.exit(1);
}

function validUrl(value) {
  return /^(stun|turns?):/i.test(value) && !/[\r\n\0]/.test(value) && value.length <= 512;
}

function parseIpv4(value) {
  const parts = String(value || "").split(".");
  if (parts.length !== 4) return null;
  const octets = parts.map((part) => {
    if (!/^(?:0|[1-9][0-9]{0,2})$/.test(part)) return NaN;
    return Number(part);
  });
  if (octets.some((octet) => !Number.isInteger(octet) || octet < 0 || octet > 255)) {
    return null;
  }
  return octets;
}

function normalizeIpv4(value, label) {
  const trimmed = String(value || "").trim();
  if (!trimmed) return "";
  if (!parseIpv4(trimmed)) fail(`${label} must be an IPv4 address`);
  return trimmed;
}

function normalizePrefix(value) {
  const trimmed = String(value || "").trim();
  if (!trimmed) return "";
  if (!/^(?:[1-9]|[12][0-9]|3[0-2])$/.test(trimmed)) {
    fail("ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX must be 1..32");
  }
  return trimmed;
}

function turnIpv4HostFromUrl(value) {
  const match = String(value || "").trim().match(/^turns?:([0-9.]+)(?::|\?|$)/i);
  if (!match || !parseIpv4(match[1])) return "";
  return match[1];
}

function firstTurnIpv4Host(iceServers) {
  for (const server of iceServers) {
    for (const url of server.urls || []) {
      const host = turnIpv4HostFromUrl(url);
      if (host) return host;
    }
  }
  return "";
}

function deriveGuestIpv4(hostIpv4) {
  const octets = parseIpv4(hostIpv4);
  if (!octets) return "";
  octets[3] = octets[3] === 2 ? 3 : 2;
  return octets.join(".");
}

function cmdlineValue(name) {
  let raw = "";
  try {
    raw = fs.readFileSync("/proc/cmdline", "utf8");
  } catch {
    return "";
  }
  for (const token of raw.trim().split(/\s+/)) {
    if (token.startsWith(`${name}=`)) return token.slice(name.length + 1);
  }
  return "";
}

function decodeHexJson(value) {
  if (!value) return {};
  if (!/^(?:[0-9a-f]{2})+$/i.test(value) || value.length > 8192) {
    fail("elastos.browser_ice_config_hex is invalid");
  }
  const json = Buffer.from(value, "hex").toString("utf8");
  try {
    const parsed = JSON.parse(json);
    return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? parsed : {};
  } catch (error) {
    fail(`elastos.browser_ice_config_hex is invalid JSON: ${error.message}`);
  }
}

function normalizeEntry(entry) {
  if (typeof entry === "string") {
    const url = entry.trim();
    if (!validUrl(url)) fail("ICE server URLs must use stun:, turn:, or turns:");
    return { urls: [url] };
  }
  if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
    fail("ICE server entries must be URL strings or RTCIceServer objects");
  }
  const urls = Array.isArray(entry.urls) ? entry.urls : [entry.urls];
  const normalizedUrls = urls
    .map((url) => String(url || "").trim())
    .filter(Boolean);
  if (normalizedUrls.length === 0 || normalizedUrls.length > 8) {
    fail("ICE server entries must contain 1..8 URLs");
  }
  for (const url of normalizedUrls) {
    if (!validUrl(url)) fail("ICE server URLs must use stun:, turn:, or turns:");
  }
  const normalized = { urls: normalizedUrls };
  if (typeof entry.username === "string" && entry.username !== "") {
    if (/[\r\n\0]/.test(entry.username)) fail("ICE username contains control characters");
    normalized.username = entry.username;
  }
  if (typeof entry.credential === "string" && entry.credential !== "") {
    if (/[\r\n\0]/.test(entry.credential)) fail("ICE credential contains control characters");
    normalized.credential = entry.credential;
  }
  return normalized;
}

const entries = [];
const bootIceConfig = decodeHexJson(cmdlineValue("elastos.browser_ice_config_hex"));
const displayMode = cmdlineValue("elastos.browser_display_mode") || "webrtc_remote_display";
function configValue(name) {
  return process.env[name] || bootIceConfig[name] || "";
}

if (configValue("ELASTOS_BROWSER_VM_ICE_SERVER")) {
  entries.push(configValue("ELASTOS_BROWSER_VM_ICE_SERVER"));
}
if (configValue("ELASTOS_BROWSER_VM_ICE_SERVERS_JSON")) {
  let parsed;
  try {
    parsed = JSON.parse(configValue("ELASTOS_BROWSER_VM_ICE_SERVERS_JSON"));
  } catch (error) {
    fail(`ELASTOS_BROWSER_VM_ICE_SERVERS_JSON is invalid JSON: ${error.message}`);
  }
  if (!Array.isArray(parsed)) {
    fail("ELASTOS_BROWSER_VM_ICE_SERVERS_JSON must be an array");
  }
  entries.push(...parsed);
}
if (entries.length > 8) fail("at most 8 ICE server entries are supported");

const credential = configValue("ELASTOS_BROWSER_VM_ICE_CREDENTIAL");
const username = configValue("ELASTOS_BROWSER_VM_ICE_USERNAME");
if ((username && !credential) || (!username && credential)) {
  fail("ELASTOS_BROWSER_VM_ICE_USERNAME and ELASTOS_BROWSER_VM_ICE_CREDENTIAL must be set together");
}
if (/[\r\n\0]/.test(username) || /[\r\n\0]/.test(credential)) {
  fail("ICE credentials must not contain control characters");
}

const iceServers = entries.map(normalizeEntry);
const hasTurnServer = iceServers.some((server) => server.urls.some((url) => /^turns?:/i.test(url)));
if (!hasTurnServer) {
  fail("webrtc_remote_display requires at least one turn:/turns: ICE server for media_transport=runtime_relay");
}
if (username && credential) {
  for (const server of iceServers) {
    if (server.urls.some((url) => /^turns?:/i.test(url))) {
      server.username ||= username;
      server.credential ||= credential;
    }
  }
}

const policy = configValue("ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY") || "relay";
if (!["all", "relay"].includes(policy)) {
  fail("ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY must be all or relay");
}
const effectivePolicy = policy;
const relayHostIpv4 = normalizeIpv4(
  configValue("ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4") || firstTurnIpv4Host(iceServers),
  "ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4",
);
const relayGuestIpv4 = normalizeIpv4(
  configValue("ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4") || deriveGuestIpv4(relayHostIpv4),
  "ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4",
);
const relayPrefix = normalizePrefix(
  configValue("ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX") || (relayHostIpv4 && relayGuestIpv4 ? "24" : ""),
);

fs.writeFileSync(
  "/run/elastos/browser-ice-servers.json",
  `${JSON.stringify(iceServers)}\n`,
);
fs.writeFileSync(
  "/run/elastos/browser-ice-transport-policy",
  `${effectivePolicy}\n`,
);
fs.writeFileSync(
  "/run/elastos/browser-rtc.json",
  `${JSON.stringify({
    iceServers,
    iceTransportPolicy: effectivePolicy,
    blockStatus: "NOT_BLOCKED",
  })}\n`,
);
fs.writeFileSync(
  "/run/elastos/browser-media-relay-network.json",
  `${JSON.stringify({
    hostIpv4: relayHostIpv4 || null,
    guestIpv4: relayGuestIpv4 || null,
    prefix: relayPrefix || null,
  })}\n`,
);
fs.writeFileSync(
  "/run/elastos/browser-media-relay-network.env",
  [
    `ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4=${relayHostIpv4}`,
    `ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4=${relayGuestIpv4}`,
    `ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX=${relayPrefix}`,
    "",
  ].join("\n"),
);
NODE

ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY="$(cat /run/elastos/browser-ice-transport-policy)"
export ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY
selkies_checkpoint "ice config written"
patch_selkies_relay_policy
selkies_checkpoint "selkies relay policy patched"
log_media_relay_config

if [ "$(cat /run/elastos/browser-ice-servers.json)" != "[]" ]; then
  setup_media_relay_network
fi
selkies_checkpoint "media relay network initialized"

selkies_checkpoint "starting selkies service"
"$@" \
  --addr=127.0.0.1 \
  --port="$ELASTOS_BROWSER_VM_SELKIES_PORT" \
  --web_root=/opt/gst-web \
  --rtc_config_json=/run/elastos/browser-rtc.json \
  --turn_shared_secret= \
  --turn_host= \
  --turn_port= \
  --stun_host= \
  --stun_port= \
  --enable_clipboard=true \
  --enable_resize=true \
  --enable_basic_auth=true \
  --basic_auth_user "$ELASTOS_BROWSER_VM_SELKIES_AUTH_USER" \
  --basic_auth_password "$ELASTOS_BROWSER_VM_SELKIES_AUTH_PASSWORD" \
  --encoder="$ELASTOS_BROWSER_VM_SELKIES_ENCODER" \
  --framerate="$ELASTOS_BROWSER_VM_SELKIES_FRAMERATE" \
  --video_bitrate="$ELASTOS_BROWSER_VM_SELKIES_VIDEO_BITRATE" \
  --audio_bitrate="$ELASTOS_BROWSER_VM_SELKIES_AUDIO_BITRATE" \
  --audio_channels="$ELASTOS_BROWSER_VM_SELKIES_AUDIO_CHANNELS" >"$ELASTOS_BROWSER_VM_LOG_DIR/browser-vm-selkies.log" 2>&1 &
selkies_checkpoint "selkies service started"

/opt/elastos/bin/node - "$ELASTOS_BROWSER_VM_CDP_PORT" "$ELASTOS_BROWSER_VM_SELKIES_PORT" <<'NODE' &
const http = require("node:http");
const [cdpPort, selkiesPort] = process.argv.slice(2);
function waitHttp(port, path, accepted) {
  const deadline = Date.now() + 60000;
  return new Promise((resolve, reject) => {
    const tick = () => {
      if (Date.now() > deadline) {
        reject(new Error(`timed out waiting for 127.0.0.1:${port}${path}`));
        return;
      }
      const req = http.request({ host: "127.0.0.1", port, path, method: "GET", timeout: 1000 }, (res) => {
        res.resume();
        res.on("end", () => accepted(res.statusCode || 0) ? resolve() : setTimeout(tick, 250));
      });
      req.on("timeout", () => req.destroy());
      req.on("error", () => setTimeout(tick, 250));
      req.end();
    };
    tick();
  });
}
(async () => {
  await waitHttp(cdpPort, "/json/version", (status) => status >= 200 && status < 300);
  await waitHttp(selkiesPort, "/health", (status) => status === 200 || status === 401);
})().catch((error) => {
  console.error(error && error.message ? error.message : String(error));
  process.exit(1);
});
NODE

cat > /run/elastos/browser-selkies-control.json <<JSON
{
  "schema": "elastos.browser.selkies-control.config/v1",
  "control_socket_path": "${ELASTOS_BROWSER_VM_CONTROL_SOCKET}",
  "replace_existing_socket": true,
  "stack_ready_timeout_ms": 60000,
  "connect_timeout_ms": 20000,
  "signal_timeout_ms": 20000,
  "selkies_ws_url": "ws://127.0.0.1:${ELASTOS_BROWSER_VM_SELKIES_PORT}/ws",
  "runtime_fetch_proxy_url": "${ELASTOS_BROWSER_NATIVE_PROXY_URL}",
  "browser_control": {
    "kind": "cdp_http",
    "endpoint": "http://127.0.0.1:${ELASTOS_BROWSER_VM_CDP_PORT}",
    "timeout_ms": ${ELASTOS_BROWSER_VM_CDP_TIMEOUT_MS}
  },
  "basic_auth": {
    "user": "${ELASTOS_BROWSER_VM_SELKIES_AUTH_USER}",
    "password": "${ELASTOS_BROWSER_VM_SELKIES_AUTH_PASSWORD}"
  },
  "ice_servers": $(cat /run/elastos/browser-ice-servers.json),
  "display_surface": {
    "stream_width": ${ELASTOS_BROWSER_VM_WIDTH},
    "stream_height": ${ELASTOS_BROWSER_VM_HEIGHT},
    "css_width": ${ELASTOS_BROWSER_VM_WIDTH},
    "css_height": ${ELASTOS_BROWSER_VM_HEIGHT}
  }
}
JSON
selkies_checkpoint "control config written"
SH
chmod 755 "$staging_dir/opt/elastos/bin/browser-vm-selkies-start"

"$repo_root/scripts/browser-vm-target-preflight.sh" --target-dir "$staging_dir" >/tmp/elastos-browser-vm-target-preflight.$$.json
preflight_output="$(cat /tmp/elastos-browser-vm-target-preflight.$$.json)"
rm -f /tmp/elastos-browser-vm-target-preflight.$$.json

if [[ -n "$rootfs_ext4" ]]; then
  command -v mke2fs >/dev/null 2>&1 || die "mke2fs not found. Install e2fsprogs."
  mkdir -p "$(dirname "$rootfs_ext4")"
  mke2fs -q -t ext4 -d "$staging_dir" -F "$rootfs_ext4" "$rootfs_size" \
    || die "mke2fs failed"
fi

python3 - "$preflight_output" "$staging_dir" "${rootfs_ext4}" <<'PY'
import json
import pathlib
import sys

preflight = json.loads(sys.argv[1])
rootfs = sys.argv[2]
rootfs_ext4 = sys.argv[3]
print(json.dumps({
    "schema": "elastos.browser.vm-target-stage/v1",
    "ok": True,
    "rootfs": rootfs,
    "rootfs_ext4": rootfs_ext4 or None,
    "preflight": preflight,
}, separators=(",", ":")))
PY
