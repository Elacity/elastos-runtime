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

OUTPUT="$output" python3 - "$repo_root" "$tmp_dir" "$target" <<'PYTEST'
import copy
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys

repo, scratch, target = map(pathlib.Path, sys.argv[1:])
data = scratch / "data"
image = data / "browser-vm/rootfs.ext4"
sidecar = image.with_name("browser-vm-rootfs-manifest.json")
preflight = json.loads(os.environ["OUTPUT"])["rootfs_contract"]["preflight"]
env = {key: value for key, value in os.environ.items()
       if not key.startswith("ELASTOS_BROWSER_VM_")}
env.update(ELASTOS_BROWSER_VM_PLATFORM="darwin-arm64",
           ELASTOS_BROWSER_VM_DATA_DIR=str(data))
debugfs = os.environ.get("ELASTOS_DEBUGFS_BIN") or shutil.which("debugfs")
mke2fs = shutil.which("mke2fs")
checks = []


def receipt():
    return {"schema": "elastos.browser.vm-rootfs-build/v1", "ok": True,
            "target_platform": "linux-arm64", "size": image.stat().st_size,
            "sha256": hashlib.sha256(image.read_bytes()).hexdigest(),
            "preflight": copy.deepcopy(preflight)}


def check(name, manifest, expected, inspector=None, error=None):
    if manifest is None:
        sidecar.unlink(missing_ok=True)
    else:
        sidecar.write_text(json.dumps(manifest))
    proc = subprocess.run([str(repo / "scripts/browser-vm-artifact-preflight.sh")],
                          env={**env, "ELASTOS_DEBUGFS_BIN": inspector or str(scratch / "absent-debugfs")},
                          capture_output=True, text=True, timeout=15)
    result = json.loads(proc.stdout)
    contract = result["rootfs_contract"]
    assert contract["ok"] is expected, (name, result)
    assert result["local_substrate_artifacts_ready"] is expected, (name, result)
    assert proc.returncode == (0 if expected else 1), (name, result, proc.stderr)
    if error:
        assert any(error in item for item in contract["errors"]), (name, result)
    checks.append(name)
    return contract


image.write_bytes(b"disposable manifest validation fixture")
valid = receipt()
check("valid receipt without debugfs", valid, True)
check("missing receipt", None, False, error="sidecar missing")
check("non-object receipt", [], False, error="JSON object")
check("bounded receipt", {"padding": "x" * (1024 * 1024)}, False, error="1 MiB")
check("wrong architecture", {**valid, "target_platform": "linux-amd64"}, False, error="target_platform")
check("wrong size", {**valid, "size": 1}, False, error="image size")
check("wrong digest", {**valid, "sha256": "0" * 64}, False, error="sha256 does not match")
check("invalid digest", {**valid, "sha256": ["0" * 64]}, False, error="SHA-256 digest")
missing_dependency = copy.deepcopy(valid)
del missing_dependency["preflight"]["required"]["chromium"]
check("missing guest dependency evidence", missing_dependency, False, error="requires chromium")
missing_audio = copy.deepcopy(valid)
missing_audio["preflight"]["optional_audio"]["pipewire"]["ok"] = False
check("contradictory audio evidence", missing_audio, False, error="requires pipewire")

if debugfs and mke2fs:
    image.unlink()
    subprocess.run([mke2fs, "-q", "-t", "ext4", "-d", str(target), "-F", str(image), "64M"],
                   check=True, capture_output=True, text=True, timeout=30)
    valid = receipt()
    contract = check("real ext4 with verified receipt", valid, True, debugfs)
    assert contract["source_kind"] == "ext4_image" and contract["inspectable"] is True
    assert contract["verified_sidecar"] is True and contract["audio_default_ready"] is True
    check("same real ext4 without debugfs", valid, True)
    check("real ext4 missing receipt with debugfs", None, False, debugfs, "sidecar missing")
    check("real ext4 wrong architecture with debugfs", {**valid, "target_platform": "linux-amd64"},
          False, debugfs, "target_platform")
    # Change an unused final byte so guest-file checks alone would still pass.
    with image.open("r+b") as handle:
        handle.seek(-1, 2)
        previous = handle.read(1)
        handle.seek(-1, 2)
        handle.write(bytes([previous[0] ^ 1]))
    for inspector in (debugfs, None):
        check(f"changed real ext4 (debugfs={bool(inspector)})", valid, False,
              inspector, "sha256 does not match")
    # A valid identity still needs intact guest files when direct inspection is available.
    subprocess.run([debugfs, "-w", "-R", "rm /opt/elastos/bin/chromium", str(image)],
                   check=True, capture_output=True, text=True, timeout=10)
    contract = check("verified identity with missing guest file", receipt(), False, debugfs)
    assert "chromium" in contract["missing"] and contract["verified_sidecar"] is True

print(json.dumps({"schema": "elastos.browser.vm-artifact-preflight-smoke/v1", "ok": True,
                  "checks": checks, "real_ext4_checked": bool(debugfs and mke2fs)}))
PYTEST
