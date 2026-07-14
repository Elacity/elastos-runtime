#!/usr/bin/env bash
set -euo pipefail

platform="${ELASTOS_BROWSER_VM_PLATFORM:-}"
if [[ -z "$platform" ]]; then
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) platform="linux-amd64" ;;
    Linux-aarch64|Linux-arm64) platform="linux-arm64" ;;
    Darwin-arm64) platform="darwin-arm64" ;;
    *) platform="$(uname -s)-$(uname -m)" ;;
  esac
fi

if [[ -n "${ELASTOS_BROWSER_VM_DATA_DIR:-}" ]]; then
  data_dir="${ELASTOS_BROWSER_VM_DATA_DIR}"
elif [[ "$(uname -s)" == "Darwin" ]]; then
  data_dir="${HOME}/Library/Application Support/elastos"
else
  data_dir="${XDG_DATA_HOME:-${HOME}/.local/share}/elastos"
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

control_socket="${ELASTOS_BROWSER_VM_CONTROL_SOCKET:-}"
rootfs="${ELASTOS_BROWSER_VM_ROOTFS:-${data_dir}/browser-vm/rootfs.ext4}"
rootfs_manifest="${ELASTOS_BROWSER_VM_ROOTFS_MANIFEST:-${data_dir}/browser-vm/browser-vm-rootfs-manifest.json}"
staged_rootfs="${ELASTOS_BROWSER_VM_STAGED_ROOTFS:-}"
crosvm="${ELASTOS_BROWSER_VM_CROSVM_BIN:-${data_dir}/bin/crosvm}"
kernel="${ELASTOS_BROWSER_VM_KERNEL:-${data_dir}/bin/vmlinux}"
vz_supervisor="${ELASTOS_BROWSER_VM_VZ_SUPERVISOR:-${data_dir}/bin/browser-vz-engine-supervisor}"
control_service="${ELASTOS_BROWSER_VM_CONTROL_SERVICE:-${data_dir}/bin/browser-vm-control-service}"
engine_supervisor="${ELASTOS_BROWSER_VM_ENGINE_SUPERVISOR:-${data_dir}/bin/browser-vm-engine-supervisor}"
target_preflight="${ELASTOS_BROWSER_VM_TARGET_PREFLIGHT:-${repo_root}/scripts/browser-vm-target-preflight.sh}"
debugfs_bin="${ELASTOS_DEBUGFS_BIN:-$(command -v debugfs 2>/dev/null || true)}"

python3 - \
  "$platform" \
  "$data_dir" \
  "$control_socket" \
  "$rootfs" \
  "$rootfs_manifest" \
  "$staged_rootfs" \
  "$crosvm" \
  "$kernel" \
  "$vz_supervisor" \
  "$control_service" \
  "$engine_supervisor" \
  "$target_preflight" \
  "$debugfs_bin" <<'PY'
import json
import hashlib
import os
import pathlib
import stat as stat_mod
import subprocess
import sys

(
    platform,
    data_dir,
    control_socket,
    rootfs,
    rootfs_manifest,
    staged_rootfs,
    crosvm,
    kernel,
    vz_supervisor,
    control_service,
    engine_supervisor,
    target_preflight,
    debugfs_bin,
) = sys.argv[1:]

REQUIRED_ROOTFS_FILES = {
    "manifest": "/etc/elastos/browser-vm-target.json",
    "init": "/opt/elastos/bin/browser-vm-init",
    "native_proxy": "/opt/elastos/bin/browser-native-proxy-engine",
    "runtime_relay": "/opt/elastos/bin/browser-vm-runtime-relay",
    "guest_control_bridge": "/opt/elastos/bin/browser-vm-guest-control-bridge",
    "control_service": "/opt/elastos/bin/browser-selkies-control-service.mjs",
    "selkies_start": "/opt/elastos/bin/browser-vm-selkies-start",
    "node": "/opt/elastos/bin/node",
    "chromium": "/opt/elastos/bin/chromium",
    "xvfb": "/usr/bin/Xvfb",
    "python3": "/usr/bin/python3",
    "gst_inspect": "/usr/bin/gst-inspect-1.0",
}

AUDIO_ROOTFS_FILES = {
    "pipewire": "/usr/bin/pipewire",
    "pipewire_pulse": "/usr/bin/pipewire-pulse",
    "wireplumber": "/usr/bin/wireplumber",
    "pw_cli": "/usr/bin/pw-cli",
}

def audio_default_ready(optional_audio):
    return bool(optional_audio) and all(
        isinstance(entry, dict) and entry.get("ok") is True
        for entry in optional_audio.values()
    )

EXPECTED_MANIFEST = {
    "schema": "elastos.browser.vm-target/v1",
    "engine": "chromium_microvm",
    "network_mode": "runtime_net_only",
    "direct_network": False,
    "wallet_injection": False,
    "media_transport": "runtime_relay",
    "display_mode": "webrtc_remote_display",
    "guarantee_level": "mechanism_microvm",
}

INIT_SNIPPETS = [
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
]

FORBIDDEN_INIT_SNIPPETS = [
    "console_device",
    "console=*)",
    "/dev/hvc0 /dev/ttyS0 /dev/console",
]

SELKIES_SNIPPETS = [
    "browser-native-proxy-engine",
    "browser-selkies-control.json",
    "selkies_checkpoint()",
    "dependencies checked",
    "runtime_net_only",
    "--proxy-server={proxy_url}",
    "--host-resolver-rules=MAP * ~NOTFOUND",
    "--web_root=/opt/gst-web",
    "ELASTOS_BROWSER_VM_SELKIES_ENCODER",
    "--encoder=\"$ELASTOS_BROWSER_VM_SELKIES_ENCODER\"",
    "PipeWire is required for Browser audio",
    "pipewire-pulse is required for Browser audio",
    "WirePlumber is required for Browser audio",
    "pw-cli is required for Browser audio",
    "configure_browser_wireplumber_headless",
    "browser-vm-wireplumber-config.log",
    'alsa_monitor.properties["alsa.reserve"] = false',
    'bluez_monitor.properties["with-logind"] = false',
    "support.logind = disabled",
    'pulsesrc.set_property("device", "auto_null.monitor")',
    "gst-inspect-1.0 pulsesrc",
]


def expected_rootfs_target():
    if platform in {"linux-arm64", "darwin-arm64"}:
        return "linux-arm64"
    if platform == "linux-amd64":
        return "linux-amd64"
    return None


def path_stat(path, executable=False, socket=False):
    if not path:
        return {"ok": False, "path": ""}
    p = pathlib.Path(path)
    ok = p.exists()
    if ok and executable:
        ok = os.access(p, os.X_OK)
    if ok and socket:
        try:
            ok = stat_mod.S_ISSOCK(p.stat().st_mode)
        except OSError:
            ok = False
    return {"ok": bool(ok), "path": path}


def run_target_preflight(target_dir):
    if not pathlib.Path(target_preflight).is_file():
        return {
            "ok": False,
            "inspectable": False,
            "source_kind": "staged_dir",
            "source": target_dir,
            "errors": [f"target preflight not found: {target_preflight}"],
        }
    proc = subprocess.run(
        [target_preflight, "--target-dir", target_dir, "--require-runtime-deps"],
        text=True,
        capture_output=True,
        check=False,
    )
    try:
        parsed = json.loads(proc.stdout.strip() or "{}")
    except Exception as exc:
        return {
            "ok": False,
            "inspectable": True,
            "source_kind": "staged_dir",
            "source": target_dir,
            "errors": [f"target preflight output was not JSON: {exc}", proc.stderr.strip()],
        }
    return {
        "ok": bool(parsed.get("ok")),
        "inspectable": True,
        "source_kind": "staged_dir",
        "source": target_dir,
        "preflight": parsed,
        "optional_audio": parsed.get("optional_audio") if isinstance(parsed.get("optional_audio"), dict) else {},
        "audio_default_ready": parsed.get("audio_default_ready"),
        "errors": [] if parsed.get("ok") else ["staged rootfs target preflight failed"],
    }


def debugfs(command, image):
    proc = subprocess.run(
        [debugfs_bin, "-R", command, image],
        text=True,
        capture_output=True,
        check=False,
    )
    return proc.returncode, proc.stdout, proc.stderr


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def ext4_has(image, guest_path):
    code, stdout, stderr = debugfs(f"stat {guest_path}", image)
    if code != 0:
        return False
    output = "\n".join(part for part in [stdout, stderr] if part)
    lowered = output.lower()
    return "file not found" not in lowered and "not found" not in lowered and "does not exist" not in lowered


def ext4_cat(image, guest_path):
    code, stdout, stderr = debugfs(f"cat {guest_path}", image)
    if code != 0:
        raise RuntimeError(stderr.strip() or stdout.strip() or f"debugfs cat failed for {guest_path}")
    return stdout


def validate_manifest(manifest, errors):
    if not isinstance(manifest, dict):
        errors.append("manifest must be a JSON object")
        return
    for key, expected in EXPECTED_MANIFEST.items():
        if manifest.get(key) != expected:
            errors.append(f"manifest.{key} must be {expected!r}")
    if manifest.get("runtime_exit_transport") not in {"carrier_stream", "vsock_relay"}:
        errors.append("manifest.runtime_exit_transport must be carrier_stream or vsock_relay")
    if manifest.get("display_backend") not in {"vm_selkies_gstreamer_webrtc", "vm_native_webrtc"}:
        errors.append("manifest.display_backend must be vm_selkies_gstreamer_webrtc or vm_native_webrtc")
    if manifest.get("control_transport") != "vsock_relay":
        errors.append("manifest.control_transport must be vsock_relay")
    if manifest.get("control_port") != 19092:
        errors.append("manifest.control_port must be 19092")


def validate_script(name, text, snippets, errors, forbidden=()):
    for snippet in snippets:
        if snippet not in text:
            errors.append(f"{name} must reference {snippet}")
    for snippet in forbidden:
        if snippet in text:
            errors.append(f"{name} must not reference stale console discovery {snippet}")
    if name == "browser-vm-selkies-start" and not any(
        snippet in text for snippet in ["selkies-gstreamer", "python3 -m selkies_gstreamer", "python3 -m selkies"]
    ):
        errors.append(f"{name} must launch Selkies GStreamer")


def inspect_ext4_sidecar_manifest(image):
    result = {
        "ok": False,
        "inspectable": False,
        "verified_sidecar": False,
        "source_kind": "ext4_image_manifest",
        "source": image,
        "manifest_path": rootfs_manifest,
        "required": {},
        "optional_audio": {},
        "audio_default_ready": False,
        "missing": [],
        "errors": [],
    }
    image_path = pathlib.Path(image)
    manifest_path = pathlib.Path(rootfs_manifest)
    if not image_path.is_file():
        result["errors"].append("rootfs image missing")
        return result
    if not manifest_path.is_file():
        result["errors"].append("rootfs manifest sidecar missing")
        return result

    try:
        manifest = json.loads(manifest_path.read_text())
    except Exception as exc:
        result["errors"].append(f"rootfs manifest sidecar JSON invalid: {exc}")
        return result

    if manifest.get("schema") != "elastos.browser.vm-rootfs-build/v1":
        result["errors"].append("rootfs manifest sidecar has wrong schema")
    if manifest.get("ok") is not True:
        result["errors"].append("rootfs manifest sidecar is not marked ok")

    target_platform = expected_rootfs_target()
    if target_platform and manifest.get("target_platform") != target_platform:
        result["errors"].append(f"rootfs manifest target_platform must be {target_platform}")

    actual_size = image_path.stat().st_size
    if manifest.get("size") != actual_size:
        result["errors"].append(f"rootfs manifest size {manifest.get('size')!r} does not match image size {actual_size}")

    expected_sha256 = manifest.get("sha256")
    if not expected_sha256:
        result["errors"].append("rootfs manifest sidecar missing sha256")
    elif sha256_file(image) != expected_sha256:
        result["errors"].append("rootfs image sha256 does not match sidecar manifest")

    preflight = manifest.get("preflight")
    if not isinstance(preflight, dict):
        result["errors"].append("rootfs manifest sidecar missing target preflight")
        preflight = {}
    elif preflight.get("ok") is not True:
        result["errors"].append("rootfs manifest target preflight is not ok")

    result["required"] = preflight.get("required") if isinstance(preflight.get("required"), dict) else {}
    result["optional_audio"] = preflight.get("optional_audio") if isinstance(preflight.get("optional_audio"), dict) else {}
    result["audio_default_ready"] = preflight.get("audio_default_ready")
    if not isinstance(result["audio_default_ready"], bool):
        result["audio_default_ready"] = audio_default_ready(result["optional_audio"])
    result["missing"] = preflight.get("missing") if isinstance(preflight.get("missing"), list) else []
    result["manifest"] = preflight.get("manifest") if isinstance(preflight.get("manifest"), dict) else {}
    result["preflight"] = preflight

    if result["audio_default_ready"] is not True:
        result["errors"].append("rootfs manifest target preflight reports audio_default_ready=false")
    if result["missing"]:
        result["errors"].append("rootfs manifest target preflight reports missing files")
    if preflight.get("manifest_errors"):
        result["errors"].append("rootfs manifest target preflight reports manifest errors")
    if preflight.get("script_errors"):
        result["errors"].append("rootfs manifest target preflight reports script errors")
    validate_manifest(result["manifest"], result["errors"])

    result["verified_sidecar"] = not result["errors"]
    result["ok"] = result["verified_sidecar"]
    return result


def inspect_ext4_rootfs(image):
    result = {
        "ok": False,
        "inspectable": False,
        "source_kind": "ext4_image",
        "source": image,
        "debugfs": path_stat(debugfs_bin, executable=True),
        "required": {},
        "optional_audio": {},
        "audio_default_ready": False,
        "missing": [],
        "errors": [],
    }
    if not pathlib.Path(image).is_file():
        result["errors"].append("rootfs image missing")
        return result
    if not debugfs_bin or not pathlib.Path(debugfs_bin).exists():
        result = inspect_ext4_sidecar_manifest(image)
        result["debugfs"] = path_stat(debugfs_bin, executable=True)
        return result

    result["inspectable"] = True
    for name, guest_path in REQUIRED_ROOTFS_FILES.items():
        ok = ext4_has(image, guest_path)
        result["required"][name] = {"ok": ok, "path": guest_path}
        if not ok:
            result["missing"].append(name)
    for name, guest_path in AUDIO_ROOTFS_FILES.items():
        ok = ext4_has(image, guest_path)
        result["optional_audio"][name] = {"ok": ok, "path": guest_path}
        if not ok:
            result["missing"].append(name)
    result["audio_default_ready"] = audio_default_ready(result["optional_audio"])

    manifest = {}
    if "manifest" not in result["missing"]:
        try:
            manifest = json.loads(ext4_cat(image, REQUIRED_ROOTFS_FILES["manifest"]))
        except Exception as exc:
            result["errors"].append(f"manifest JSON invalid: {exc}")
    validate_manifest(manifest, result["errors"])
    result["manifest"] = manifest if isinstance(manifest, dict) else {}

    if "init" not in result["missing"]:
        try:
            validate_script(
                "browser-vm-init",
                ext4_cat(image, REQUIRED_ROOTFS_FILES["init"]),
                INIT_SNIPPETS,
                result["errors"],
                FORBIDDEN_INIT_SNIPPETS,
            )
        except Exception as exc:
            result["errors"].append(f"browser-vm-init unreadable: {exc}")
    if "selkies_start" not in result["missing"]:
        try:
            validate_script(
                "browser-vm-selkies-start",
                ext4_cat(image, REQUIRED_ROOTFS_FILES["selkies_start"]),
                SELKIES_SNIPPETS,
                result["errors"],
            )
        except Exception as exc:
            result["errors"].append(f"browser-vm-selkies-start unreadable: {exc}")

    result["ok"] = not result["missing"] and not result["errors"]
    return result


def inspect_rootfs():
    if staged_rootfs and pathlib.Path(staged_rootfs).is_dir():
        return run_target_preflight(staged_rootfs)
    if pathlib.Path(rootfs).is_dir():
        return run_target_preflight(rootfs)
    return inspect_ext4_rootfs(rootfs)


rootfs_contract = inspect_rootfs()

control = {
    "control_socket": path_stat(control_socket, socket=True),
    "control_service": path_stat(control_service, executable=True),
    "engine_supervisor": path_stat(engine_supervisor, executable=True),
}

if platform.startswith("linux-"):
    substrate = {
        "kind": "crosvm",
        "kvm": path_stat("/dev/kvm"),
        "crosvm": path_stat(crosvm, executable=True),
        "kernel": path_stat(kernel),
        "rootfs_contract": {"ok": rootfs_contract["ok"], "path": rootfs},
    }
elif platform.startswith("darwin-"):
    substrate = {
        "kind": "apple_virtualization_framework",
        "vz_supervisor": path_stat(vz_supervisor, executable=True),
        "kernel": path_stat(kernel),
        "rootfs_contract": {"ok": rootfs_contract["ok"], "path": rootfs},
    }
else:
    substrate = {
        "kind": "unsupported",
        "supported_platform": {"ok": False, "path": platform},
        "rootfs_contract": {"ok": rootfs_contract["ok"], "path": rootfs},
    }

missing_for_local_substrate = [
    name for name, entry in substrate.items()
    if isinstance(entry, dict) and not entry.get("ok", False)
]

local_substrate_artifacts_ready = not missing_for_local_substrate and rootfs_contract["ok"]
launch_ready = bool(control["control_socket"]["ok"])

if launch_ready:
    reason = "Browser VM control socket is available; Runtime can delegate Browser launches."
elif local_substrate_artifacts_ready:
    reason = "Browser VM artifacts are present, but no control socket is running yet."
else:
    reason = "Browser VM artifacts are incomplete; see missing_for_local_substrate and rootfs_contract."

print(json.dumps({
    "schema": "elastos.browser.vm-artifact-preflight/v1",
    "platform": platform,
    "data_dir": data_dir,
    "ok": bool(launch_ready or local_substrate_artifacts_ready),
    "launch_ready": launch_ready,
    "local_substrate_artifacts_ready": bool(local_substrate_artifacts_ready),
    "reason": reason,
    "control": control,
    "substrate": substrate,
    "missing_for_local_substrate": missing_for_local_substrate,
    "rootfs_contract": rootfs_contract,
}, separators=(",", ":")))

sys.exit(0 if (launch_ready or local_substrate_artifacts_ready) else 1)
PY
