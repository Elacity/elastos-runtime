#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

target="$tmp_dir/staged-rootfs"
mkdir -p \
  "$target/etc/elastos" \
  "$target/opt/elastos/bin" \
  "$target/usr/bin" \
  "$tmp_dir/data/bin" \
  "$tmp_dir/data/browser-vm"

cat > "$target/etc/elastos/browser-vm-target.json" <<'JSON'
{
  "schema": "elastos.browser.vm-target/v1",
  "engine": "chromium_microvm",
  "network_mode": "runtime_net_only",
  "direct_network": false,
  "wallet_injection": false,
  "media_transport": "runtime_relay",
  "display_mode": "webrtc_remote_display",
  "guarantee_level": "mechanism_microvm",
  "display_backend": "vm_selkies_gstreamer_webrtc",
  "runtime_exit_transport": "vsock_relay",
  "control_transport": "vsock_relay",
  "control_port": 19092
}
JSON

cat > "$target/opt/elastos/bin/browser-vm-init" <<'SH'
#!/bin/sh
rootfs_checkpoint() { echo "rootfs checkpoint: $*"; }
ELASTOS_BROWSER_VM_SERIAL_LOG_DEV=""
export ELASTOS_BROWSER_VM_SERIAL_LOG_DEV
rootfs_checkpoint "rootfs diagnostics initialized"
rootfs_checkpoint "runtime filesystems mounted"
modprobe virtio_net || true
/opt/elastos/bin/browser-vm-runtime-relay &
rootfs_checkpoint "starting browser stack"
/opt/elastos/bin/browser-vm-selkies-start
rootfs_checkpoint "browser control socket present"
ELASTOS_BROWSER_SELKIES_CONTROL_CONFIG="$(cat /run/elastos/browser-selkies-control.json)" \
  /opt/elastos/bin/node /opt/elastos/bin/browser-selkies-control-service.mjs &
ELASTOS_BROWSER_VM_CONTROL_BRIDGE_CONFIG="$(cat /run/elastos/browser-vm-control-bridge.json)" \
  exec /opt/elastos/bin/browser-vm-guest-control-bridge
rootfs_checkpoint "guest control bridge started"
SH
chmod 755 "$target/opt/elastos/bin/browser-vm-init"

cat > "$target/opt/elastos/bin/browser-vm-selkies-start" <<'SH'
#!/bin/sh
selkies_checkpoint() { echo "selkies checkpoint: $*"; }
selkies_checkpoint "dependencies checked"
echo runtime_net_only
echo '--proxy-server={proxy_url}'
echo '--host-resolver-rules=MAP * ~NOTFOUND'
echo 'elastos.browser.native-proxy-engine.ready/v1'
echo elastos.browser_ice_config_hex
echo patch_selkies_relay_policy
echo ice-transport-policy
echo 'webrtc_remote_display requires at least one turn:/turns:'
echo 'media relay IPv4'
echo 'ELASTOS_BROWSER_VM_SELKIES_ENCODER'
echo '--encoder="$ELASTOS_BROWSER_VM_SELKIES_ENCODER"'
echo 'PipeWire is required for Browser audio'
echo 'pipewire-pulse is required for Browser audio'
echo 'WirePlumber is required for Browser audio'
echo 'pw-cli is required for Browser audio'
echo configure_browser_wireplumber_headless
echo browser-vm-wireplumber-config.log
echo 'alsa_monitor.properties["alsa.reserve"] = false'
echo 'bluez_monitor.properties["with-logind"] = false'
echo 'support.logind = disabled'
echo start_browser_audio_stack
echo PULSE_SERVER
echo 'pulsesrc.set_property("device", "auto_null.monitor")'
echo 'gst-inspect-1.0 pulsesrc'
echo '--audio_bitrate="$ELASTOS_BROWSER_VM_SELKIES_AUDIO_BITRATE"'
echo '--audio_channels="$ELASTOS_BROWSER_VM_SELKIES_AUDIO_CHANNELS"'
echo 'self.build_audio_pipeline()'
echo 'Selkies 1.6.1 audio RTP header extensions are fragile'
echo 'forcing audio SDP offer for split product audio peer'
echo 'pulsesrc = Gst.ElementFactory.make("pulsesrc")'
echo 'opusenc = Gst.ElementFactory.make("opusenc")'
echo 'self.opusenc = opusenc'
echo 'Audio encoder is unavailable'
echo 'rtpopuspay_queue = Gst.ElementFactory.make("queue")'
echo 'Audio pipeline element is unavailable'
ELASTOS_BROWSER_VM_ICE_SERVERS_JSON='[]'
ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4='192.168.65.2'
export ELASTOS_BROWSER_VM_ICE_SERVERS_JSON
export ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4
ip addr add 192.168.65.2/24 dev eth0 || true
cat > /run/elastos/browser-rtc.json <<JSON
{
  "ice_servers": $(cat /run/elastos/browser-ice-servers.json)
}
JSON
cat > /run/elastos/browser-ice-servers.json <<JSON
[]
JSON
cat > /run/elastos/browser-ice-transport-policy <<EOF
relay
EOF
cat > /run/elastos/browser-media-relay-network.json <<JSON
{}
JSON
/opt/elastos/bin/browser-native-proxy-engine &
selkies-gstreamer --web_root=/opt/gst-web &
cat > /run/elastos/browser-selkies-control.json <<JSON
{
  "runtime_fetch_proxy_url": "http://127.0.0.1:19090"
}
JSON
SH
chmod 755 "$target/opt/elastos/bin/browser-vm-selkies-start"

for executable in \
  "$target/opt/elastos/bin/browser-native-proxy-engine" \
  "$target/opt/elastos/bin/browser-vm-runtime-relay" \
  "$target/opt/elastos/bin/node" \
  "$target/opt/elastos/bin/chromium" \
  "$target/usr/bin/Xvfb" \
  "$target/usr/bin/python3" \
  "$target/usr/bin/pipewire" \
  "$target/usr/bin/pipewire-pulse" \
  "$target/usr/bin/wireplumber" \
  "$target/usr/bin/pw-cli" \
  "$target/usr/bin/gst-inspect-1.0" \
  "$tmp_dir/data/bin/browser-vz-engine-supervisor"
do
  printf '#!/bin/sh\nexit 0\n' > "$executable"
  chmod 755 "$executable"
done
cat > "$target/opt/elastos/bin/browser-vm-guest-control-bridge" <<'SH'
#!/bin/sh
: elastos.browser.vm-guest-control-bridge.config/v1
: control_socket_ready_timeout_ms
: control_request_timeout_ms
exit 0
SH
chmod 755 "$target/opt/elastos/bin/browser-vm-guest-control-bridge"
printf '#!/usr/bin/env node\n' > "$target/opt/elastos/bin/browser-selkies-control-service.mjs"
printf 'fake-kernel\n' > "$tmp_dir/data/bin/vmlinux"

output="$(ELASTOS_BROWSER_VM_PLATFORM=darwin-arm64 \
  ELASTOS_BROWSER_VM_DATA_DIR="$tmp_dir/data" \
  ELASTOS_BROWSER_VM_STAGED_ROOTFS="$target" \
  "$repo_root/scripts/browser-vm-artifact-preflight.sh")"

OUTPUT="$output" node - <<'NODE'
const result = JSON.parse(process.env.OUTPUT);
if (result.schema !== "elastos.browser.vm-artifact-preflight/v1") throw new Error("wrong schema");
if (result.local_substrate_artifacts_ready !== true) throw new Error(`staged artifacts should be ready: ${process.env.OUTPUT}`);
if (result.launch_ready !== false) throw new Error("smoke should not create a control socket");
if (result.rootfs_contract?.ok !== true) throw new Error(`rootfs contract should pass: ${process.env.OUTPUT}`);
if (result.rootfs_contract?.preflight?.optional_audio?.pipewire?.ok !== true) throw new Error("optional audio deps should be reported");
if (result.rootfs_contract?.audio_default_ready !== true) throw new Error("staged rootfs should be audio-default-ready");
if (result.substrate?.kernel?.ok !== true) throw new Error("darwin substrate must include kernel readiness");
NODE

if command -v debugfs >/dev/null 2>&1 && command -v mke2fs >/dev/null 2>&1 && [[ "$(uname -s)" == "Linux" ]]; then
  mkdir -p "$tmp_dir/bin"
  for executable in browser-native-proxy-engine browser-vm-runtime-relay node chromium; do
    cp /bin/true "$tmp_dir/bin/$executable"
    chmod 755 "$tmp_dir/bin/$executable"
  done
  cp /bin/true "$tmp_dir/bin/browser-vm-guest-control-bridge"
  printf '\nelastos.browser.vm-guest-control-bridge.config/v1\ncontrol_socket_ready_timeout_ms\ncontrol_request_timeout_ms\n' >> "$tmp_dir/bin/browser-vm-guest-control-bridge"
  chmod 755 "$tmp_dir/bin/browser-vm-guest-control-bridge"
  printf '#!/usr/bin/env node\n' > "$tmp_dir/bin/browser-selkies-control-service.mjs"
  "$repo_root/scripts/build/stage-browser-vm-target.sh" \
    --out-dir "$tmp_dir/stage" \
    --native-proxy-bin "$tmp_dir/bin/browser-native-proxy-engine" \
    --runtime-relay-bin "$tmp_dir/bin/browser-vm-runtime-relay" \
    --guest-control-bridge-bin "$tmp_dir/bin/browser-vm-guest-control-bridge" \
    --control-service "$tmp_dir/bin/browser-selkies-control-service.mjs" \
    --node-bin "$tmp_dir/bin/node" \
    --chromium-bin "$tmp_dir/bin/chromium" >/dev/null

  for executable in Xvfb python3 pipewire pipewire-pulse wireplumber pw-cli gst-inspect-1.0; do
    cp /bin/true "$tmp_dir/stage/rootfs/usr/bin/$executable"
    chmod 755 "$tmp_dir/stage/rootfs/usr/bin/$executable"
  done
  mke2fs -q -t ext4 -d "$tmp_dir/stage/rootfs" -F "$tmp_dir/data/browser-vm/rootfs.ext4" 2048M

  ext4_output="$(ELASTOS_BROWSER_VM_PLATFORM=darwin-arm64 \
    ELASTOS_BROWSER_VM_DATA_DIR="$tmp_dir/data" \
    "$repo_root/scripts/browser-vm-artifact-preflight.sh")"
  OUTPUT="$ext4_output" node - <<'NODE'
const result = JSON.parse(process.env.OUTPUT);
if (result.rootfs_contract?.source_kind !== "ext4_image") throw new Error(`expected ext4 inspection: ${process.env.OUTPUT}`);
if (result.rootfs_contract?.ok !== true) throw new Error(`ext4 rootfs contract should pass: ${process.env.OUTPUT}`);
if (result.rootfs_contract?.optional_audio?.pipewire?.ok !== true) throw new Error("ext4 optional audio deps should be reported");
if (result.rootfs_contract?.audio_default_ready !== true) throw new Error("ext4 rootfs should be audio-default-ready");
if (result.local_substrate_artifacts_ready !== true) throw new Error(`ext4 artifacts should be ready: ${process.env.OUTPUT}`);
NODE
fi

printf '%s\n' '{"schema":"elastos.browser.vm-artifact-preflight-smoke/v1","ok":true}'
