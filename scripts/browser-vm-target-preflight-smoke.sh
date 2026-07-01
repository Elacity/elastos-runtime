#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
node_bin="${ELASTOS_NODE_BIN:-}"
if [[ -z "$node_bin" ]]; then
  node_bin="$(command -v node 2>/dev/null || true)"
fi
if [[ -z "$node_bin" ]]; then
  node_bin="$HOME/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node"
fi
if [[ ! -x "$node_bin" ]]; then
  echo "node not found. Set ELASTOS_NODE_BIN to an executable node binary." >&2
  exit 2
fi

target="$tmp_dir/rootfs"
mkdir -p \
  "$target/etc/elastos" \
  "$target/opt/elastos/bin" \
  "$target/usr/bin"

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
  "runtime_exit_transport": "carrier_stream",
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
echo start_browser_audio_stack
echo PULSE_SERVER
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
  "$target/usr/bin/node" \
  "$target/usr/bin/chromium" \
  "$target/usr/bin/Xvfb" \
  "$target/usr/bin/python3" \
  "$target/usr/bin/pipewire" \
  "$target/usr/bin/pipewire-pulse" \
  "$target/usr/bin/wireplumber" \
  "$target/usr/bin/pw-cli" \
  "$target/usr/bin/gst-inspect-1.0"
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

output="$("$repo_root/scripts/browser-vm-target-preflight.sh" --target-dir "$target" --require-runtime-deps)"
OUTPUT="$output" "$node_bin" - <<'NODE'
const result = JSON.parse(process.env.OUTPUT);
if (result.schema !== "elastos.browser.vm-target-preflight/v1") throw new Error("wrong schema");
if (result.ok !== true) throw new Error(`target preflight failed: ${process.env.OUTPUT}`);
if (result.runtime_deps_required !== true) throw new Error("strict runtime dependency preflight was not enabled");
if (result.manifest?.media_transport !== "runtime_relay") throw new Error("missing runtime relay contract");
if (result.manifest?.engine !== "chromium_microvm") throw new Error("missing chromium_microvm contract");
if (result.optional_audio?.pipewire?.ok !== true) throw new Error("optional audio deps should be reported");
if (result.audio_default_ready !== true) throw new Error("audio should be default-ready when PipeWire helpers are present");
NODE

printf '%s\n' '{"schema":"elastos.browser.vm-target-preflight-smoke/v1","ok":true}'
