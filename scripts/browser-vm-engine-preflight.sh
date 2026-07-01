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

control_socket="${ELASTOS_BROWSER_VM_CONTROL_SOCKET:-}"
rootfs="${ELASTOS_BROWSER_VM_ROOTFS:-${data_dir}/browser-vm/rootfs.ext4}"
crosvm="${ELASTOS_BROWSER_VM_CROSVM_BIN:-${data_dir}/bin/crosvm}"
kernel="${ELASTOS_BROWSER_VM_KERNEL:-${data_dir}/bin/vmlinux}"
vz_supervisor="${ELASTOS_BROWSER_VM_VZ_SUPERVISOR:-${data_dir}/bin/browser-vz-engine-supervisor}"

python3 - "$platform" "$data_dir" "$control_socket" "$rootfs" "$crosvm" "$kernel" "$vz_supervisor" <<'PY'
import json
import os
import pathlib
import socket
import stat as stat_mod
import sys

platform, data_dir, control_socket, rootfs, crosvm, kernel, vz_supervisor = sys.argv[1:]

def stat(path):
    return {"ok": bool(path) and pathlib.Path(path).exists(), "path": path}

def control_socket_status(path):
    entry = stat(path)
    entry["exists"] = entry["ok"]
    entry["connect_ok"] = False
    if not entry["exists"]:
        entry["ok"] = False
        return entry
    try:
        mode = pathlib.Path(path).stat().st_mode
    except OSError as exc:
        entry["ok"] = False
        entry["error"] = str(exc)
        return entry
    if not stat_mod.S_ISSOCK(mode):
        entry["ok"] = False
        entry["error"] = "path exists but is not a Unix socket"
        return entry
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(1.5)
            client.connect(path)
            entry["connect_ok"] = True
            client.sendall(b"GET /status HTTP/1.1\r\nHost: browser-vm\r\nConnection: close\r\n\r\n")
            chunks = []
            while True:
                chunk = client.recv(4096)
                if not chunk:
                    break
                chunks.append(chunk)
                if sum(len(item) for item in chunks) >= 65536:
                    break
    except OSError as exc:
        entry["ok"] = False
        entry["error"] = str(exc)
        return entry
    text = b"".join(chunks).decode("utf-8", "replace")
    status_line = text.splitlines()[0] if text else ""
    entry["http_status"] = status_line
    entry["ok"] = status_line.startswith("HTTP/1.1 200") or status_line.startswith("HTTP/1.0 200")
    if not entry["ok"]:
        entry["error"] = "control socket did not return HTTP 200 for /status"
    return entry

def missing_names(entries):
    return [name for name, entry in entries.items() if not entry.get("ok")]

control_socket_entry = control_socket_status(control_socket)
result = {
    "schema": "elastos.browser.vm-engine-preflight/v1",
    "platform": platform,
    "data_dir": data_dir,
    "control_socket": control_socket_entry,
    "rootfs": stat(rootfs),
    "remote_control_supported": True,
    "remote_control_hint": "Set ELASTOS_BROWSER_VM_CONTROL_SOCKET to a local Unix socket served by a Browser VM control plane. That socket may be backed by a local VM substrate or by a remote VM provider reached through Runtime/Carrier/SSH tunnel plumbing.",
}

local_ready = False
missing = []
if platform.startswith("linux-"):
    local = {
        "kvm": stat("/dev/kvm"),
        "crosvm": stat(crosvm),
        "kernel": stat(kernel),
        "rootfs": result["rootfs"],
    }
    local_ready = all(entry["ok"] for entry in local.values())
    missing = missing_names(local)
    result.update({
        "substrate": "crosvm",
        **{key: value for key, value in local.items() if key != "rootfs"},
    })
elif platform.startswith("darwin-"):
    local = {
        "vz_supervisor": stat(vz_supervisor),
        "kernel": stat(kernel),
        "rootfs": result["rootfs"],
    }
    local_ready = all(entry["ok"] for entry in local.values())
    missing = missing_names(local)
    result.update({
        "substrate": "apple_virtualization_framework",
        "vz_supervisor": local["vz_supervisor"],
        "kernel": local["kernel"],
    })
else:
    result["substrate"] = "unsupported"
    missing = ["supported_platform"]

remote_ready = result["control_socket"]["ok"]
result["local_substrate_ready"] = bool(local_ready)
result["launch_ready"] = bool(remote_ready)
result["ok"] = result["launch_ready"]
result["missing_for_local_substrate"] = missing
if remote_ready:
    result["execution_mode"] = "remote_vm_control_socket"
    result["reason"] = "Browser VM control socket is available; local KVM/VZ is not required on this host."
elif local_ready:
    result["execution_mode"] = "local_substrate_ready_needs_vm_control_service"
    result["reason"] = "Local VM substrate artifacts are present, but no Browser VM control socket is configured yet."
else:
    result["execution_mode"] = "unavailable"
    if platform.startswith("linux-") and "kvm" in missing:
        result["reason"] = "Local crosvm Browser VM is unavailable because /dev/kvm is missing. This is acceptable for a gateway host if ELASTOS_BROWSER_VM_CONTROL_SOCKET points at a remote/operator Browser VM provider."
    else:
        result["reason"] = "Browser VM launch is unavailable until a control socket is configured or the local substrate is provisioned."

print(json.dumps(result, separators=(",", ":")))
PY
