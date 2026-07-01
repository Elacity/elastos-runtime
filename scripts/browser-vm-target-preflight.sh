#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  scripts/browser-vm-target-preflight.sh --target-dir /path/to/staged-rootfs [--require-runtime-deps]

Checks a staged Browser VM rootfs directory before it is packed into
rootfs.ext4. This does not launch a VM and does not prove media quality; it
proves the image contains the minimum ElastOS Browser VM contract surface.
Pass --require-runtime-deps when checking a complete rootfs image rather than
the minimal ElastOS overlay produced by stage-browser-vm-target.sh.

Required guest contract:
  /etc/elastos/browser-vm-target.json
  /opt/elastos/bin/browser-vm-init
  /opt/elastos/bin/browser-native-proxy-engine
  /opt/elastos/bin/browser-vm-runtime-relay
  /opt/elastos/bin/browser-vm-guest-control-bridge
  /opt/elastos/bin/browser-selkies-control-service.mjs
  /opt/elastos/bin/browser-vm-selkies-start
  node and Chromium in the guest filesystem

Additional --require-runtime-deps contract:
  Xvfb, python3, gst-inspect-1.0, PipeWire, pipewire-pulse, WirePlumber,
  pw-cli, and GStreamer pulsesrc. Audio is part of the default product VM
  target and missing audio support fails this preflight.
USAGE
}

target_dir=""
require_runtime_deps=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target-dir)
      target_dir="${2:-}"
      shift 2
      ;;
    --require-runtime-deps)
      require_runtime_deps=1
      shift
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

if [[ -z "$target_dir" ]]; then
  usage >&2
  exit 2
fi

python3 - "$target_dir" "$require_runtime_deps" <<'PY'
import json
import os
import pathlib
import sys

target_dir = pathlib.Path(sys.argv[1]).resolve()
require_runtime_deps = sys.argv[2] == "1"

def rel(path):
    return target_dir / path.lstrip("/")

def stat(path, executable=False):
    full = rel(path)
    ok = full.is_file() and (not executable or os.access(full, os.X_OK))
    return {"ok": ok, "path": str(full), "relative_path": path}

def first_present(paths, executable=False):
    entries = [stat(path, executable=executable) for path in paths]
    return {
        "ok": any(entry["ok"] for entry in entries),
        "candidates": entries,
    }

required = {
    "manifest": stat("/etc/elastos/browser-vm-target.json"),
    "init": stat("/opt/elastos/bin/browser-vm-init", executable=True),
    "native_proxy": stat("/opt/elastos/bin/browser-native-proxy-engine", executable=True),
    "runtime_relay": stat("/opt/elastos/bin/browser-vm-runtime-relay", executable=True),
    "guest_control_bridge": stat("/opt/elastos/bin/browser-vm-guest-control-bridge", executable=True),
    "control_service": stat("/opt/elastos/bin/browser-selkies-control-service.mjs"),
    "selkies_start": stat("/opt/elastos/bin/browser-vm-selkies-start", executable=True),
    "node": first_present([
        "/opt/elastos/bin/node",
        "/usr/bin/node",
        "/usr/local/bin/node",
    ], executable=True),
    "chromium": first_present([
        "/opt/elastos/bin/chromium",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/bin/google-chrome",
    ], executable=True),
}
if require_runtime_deps:
    required.update({
        "xvfb": first_present([
            "/usr/bin/Xvfb",
            "/bin/Xvfb",
        ], executable=True),
        "python3": first_present([
            "/usr/bin/python3",
            "/bin/python3",
        ], executable=True),
        "gst_inspect": first_present([
            "/usr/bin/gst-inspect-1.0",
            "/bin/gst-inspect-1.0",
        ], executable=True),
    })
optional_audio = {}
if require_runtime_deps:
    optional_audio = {
        "pipewire": first_present([
            "/usr/bin/pipewire",
            "/bin/pipewire",
        ], executable=True),
        "pipewire_pulse": first_present([
            "/usr/bin/pipewire-pulse",
            "/bin/pipewire-pulse",
        ], executable=True),
        "wireplumber": first_present([
            "/usr/bin/wireplumber",
            "/bin/wireplumber",
        ], executable=True),
        "pw_cli": first_present([
            "/usr/bin/pw-cli",
            "/bin/pw-cli",
        ], executable=True),
    }
audio_default_ready = None
if require_runtime_deps:
    audio_default_ready = bool(optional_audio) and all(
        entry.get("ok") is True for entry in optional_audio.values()
    )

manifest = {}
manifest_errors = []
script_errors = []
manifest_path = rel("/etc/elastos/browser-vm-target.json")
if manifest_path.is_file():
    try:
        manifest = json.loads(manifest_path.read_text())
    except Exception as exc:
        manifest_errors.append(f"manifest JSON invalid: {exc}")
else:
    manifest_errors.append("manifest missing")

expected_fields = {
    "schema": "elastos.browser.vm-target/v1",
    "engine": "chromium_microvm",
    "network_mode": "runtime_net_only",
    "direct_network": False,
    "wallet_injection": False,
    "media_transport": "runtime_relay",
    "display_mode": "webrtc_remote_display",
    "guarantee_level": "mechanism_microvm",
}
for key, expected in expected_fields.items():
    if manifest.get(key) != expected:
        manifest_errors.append(f"{key} must be {expected!r}")

if manifest.get("runtime_exit_transport") not in {"carrier_stream", "vsock_relay"}:
    manifest_errors.append("runtime_exit_transport must be carrier_stream or vsock_relay")

if manifest.get("display_backend") not in {"vm_selkies_gstreamer_webrtc", "vm_native_webrtc"}:
    manifest_errors.append("display_backend must be vm_selkies_gstreamer_webrtc or vm_native_webrtc")

if manifest.get("control_transport") != "vsock_relay":
    manifest_errors.append("control_transport must be vsock_relay")

if manifest.get("control_port") != 19092:
    manifest_errors.append("control_port must be 19092")

init_path = rel("/opt/elastos/bin/browser-vm-init")
if init_path.is_file():
    init_text = init_path.read_text(errors="replace")
    for required_snippet in [
        "browser-vm-runtime-relay",
        "browser-vm-guest-control-bridge",
        "browser-vm-selkies-start",
        "ELASTOS_BROWSER_SELKIES_CONTROL_CONFIG",
        "ELASTOS_BROWSER_VM_CONTROL_BRIDGE_CONFIG",
        "browser-selkies-control-service.mjs",
        "virtio_net",
        "rootfs_checkpoint()",
        'ELASTOS_BROWSER_VM_SERIAL_LOG_DEV=""',
        "rootfs diagnostics initialized",
        "runtime filesystems mounted",
        "starting browser stack",
        "browser control socket present",
        "guest control bridge started",
    ]:
        if required_snippet not in init_text:
            script_errors.append(f"browser-vm-init must reference {required_snippet}")
    for forbidden_snippet in [
        "console_device",
        'console=*)',
        "/dev/hvc0 /dev/ttyS0 /dev/console",
    ]:
        if forbidden_snippet in init_text:
            script_errors.append(f"browser-vm-init must not reference stale console discovery {forbidden_snippet}")
else:
    script_errors.append("browser-vm-init missing")

guest_control_bridge_path = rel("/opt/elastos/bin/browser-vm-guest-control-bridge")
if guest_control_bridge_path.is_file():
    guest_control_bridge_text = guest_control_bridge_path.read_text(errors="replace")
    for required_snippet in [
        "elastos.browser.vm-guest-control-bridge.config/v1",
        "control_socket_ready_timeout_ms",
        "control_request_timeout_ms",
    ]:
        if required_snippet not in guest_control_bridge_text:
            script_errors.append(f"browser-vm-guest-control-bridge must support {required_snippet}")
else:
    script_errors.append("browser-vm-guest-control-bridge missing")

selkies_start_path = rel("/opt/elastos/bin/browser-vm-selkies-start")
if selkies_start_path.is_file():
    selkies_text = selkies_start_path.read_text(errors="replace")
    for required_snippet in [
        "browser-native-proxy-engine",
        "browser-selkies-control.json",
        "selkies_checkpoint()",
        "runtime_fetch_proxy_url",
        "elastos.browser.native-proxy-engine.ready/v1",
        "runtime_net_only",
        "--proxy-server={proxy_url}",
        "--host-resolver-rules=MAP * ~NOTFOUND",
        "--web_root=/opt/gst-web",
        "ELASTOS_BROWSER_VM_ICE_SERVERS_JSON",
        "elastos.browser_ice_config_hex",
        "/run/elastos/browser-rtc.json",
        "/run/elastos/browser-ice-servers.json",
        "/run/elastos/browser-ice-transport-policy",
        "/run/elastos/browser-media-relay-network.json",
        "ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4",
        "ELASTOS_BROWSER_VM_SELKIES_ENCODER",
        "--encoder=\"$ELASTOS_BROWSER_VM_SELKIES_ENCODER\"",
        "PipeWire is required for Browser audio",
        "pipewire-pulse is required for Browser audio",
        "WirePlumber is required for Browser audio",
        "pw-cli is required for Browser audio",
        "start_browser_audio_stack",
        "PULSE_SERVER",
        "gst-inspect-1.0 pulsesrc",
        "--audio_bitrate=\"$ELASTOS_BROWSER_VM_SELKIES_AUDIO_BITRATE\"",
        "--audio_channels=\"$ELASTOS_BROWSER_VM_SELKIES_AUDIO_CHANNELS\"",
        "self.build_audio_pipeline()",
        "Selkies 1.6.1 audio RTP header extensions are fragile",
        "forcing audio SDP offer for split product audio peer",
        'pulsesrc = Gst.ElementFactory.make("pulsesrc")',
        'opusenc = Gst.ElementFactory.make("opusenc")',
        "self.opusenc = opusenc",
        "Audio encoder is unavailable",
        'rtpopuspay_queue = Gst.ElementFactory.make("queue")',
        "Audio pipeline element is unavailable",
        "media relay IPv4",
        "ip addr add",
        "patch_selkies_relay_policy",
        "ice-transport-policy",
        "webrtc_remote_display requires at least one turn:/turns:",
        "\"ice_servers\": $(cat /run/elastos/browser-ice-servers.json)",
    ]:
        if required_snippet not in selkies_text:
            script_errors.append(f"browser-vm-selkies-start must reference {required_snippet}")
    if not any(
        snippet in selkies_text
        for snippet in ["selkies-gstreamer", "python3 -m selkies_gstreamer", "python3 -m selkies"]
    ):
        script_errors.append("browser-vm-selkies-start must launch Selkies GStreamer")
else:
    script_errors.append("browser-vm-selkies-start missing")

missing = []
for name, entry in required.items():
    if not entry["ok"]:
        missing.append(name)
if require_runtime_deps and not audio_default_ready:
    for name, entry in optional_audio.items():
        if not entry["ok"]:
            missing.append(name)

result = {
    "schema": "elastos.browser.vm-target-preflight/v1",
    "target_dir": str(target_dir),
    "runtime_deps_required": require_runtime_deps,
    "ok": not missing and not manifest_errors and not script_errors,
    "missing": missing,
    "manifest_errors": manifest_errors,
    "script_errors": script_errors,
    "required": required,
    "optional_audio": optional_audio,
    "audio_default_ready": audio_default_ready,
    "manifest": manifest if isinstance(manifest, dict) else {},
}
print(json.dumps(result, separators=(",", ":")))
sys.exit(0 if result["ok"] else 1)
PY
