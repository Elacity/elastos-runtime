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
    # Target maintenance must report guest drift before touching host helpers
    # or rewriting the image/receipt. Use a real cpio initrd with an old helper.
    import gzip
    initrd_tree = scratch / "initrd-tree"
    (initrd_tree / "bin").mkdir(parents=True)
    (initrd_tree / "bin/browser-selkies-control-service.mjs").write_text("old control helper")
    packed = subprocess.run(["cpio", "-o", "--format=newc"], cwd=initrd_tree,
        input=b"bin/browser-selkies-control-service.mjs\n", capture_output=True, check=True).stdout
    for rel in ["bin/initrd", "browser-vm/initrd"]:
        (data / rel).write_bytes(gzip.compress(packed))
    maintenance_receipt = copy.deepcopy(valid)
    for name, rel in [("kernel", "bin/vmlinux"), ("initrd", "bin/initrd")]:
        artifact = data / rel
        maintenance_receipt[name] = {"size": artifact.stat().st_size,
                                    "sha256": hashlib.sha256(artifact.read_bytes()).hexdigest()}
    sidecar.write_text(json.dumps(maintenance_receipt))
    preserved = {p: hashlib.sha256(p.read_bytes()).hexdigest()
                 for p in [image, sidecar, data / "bin/initrd", data / "browser-vm/initrd"]}
    refresh = subprocess.run([str(repo / "scripts/browser-vm-target-refresh.sh"),
        "--source-dir", str(repo), "--data-dir", str(data)],
        env={**env, "ELASTOS_DEBUGFS_BIN":debugfs}, capture_output=True, text=True, timeout=30)
    assert refresh.returncode == 1 and "Rebuild and install the complete image set" in refresh.stderr, (refresh.stdout, refresh.stderr)
    assert all(hashlib.sha256(p.read_bytes()).hexdigest() == digest for p,digest in preserved.items())
    assert not (data / "scripts").exists() and not (data / "backups").exists()
    checks.append("guest drift preserves real ext4, initrd, receipt and host helpers")
    sidecar.write_text(json.dumps(valid))
    # The VZ host fixture is platform-specific; ext4 integrity runs on both hosts.
    import platform as host_platform
    if host_platform.system() == "Darwin" and host_platform.machine() == "arm64":
        # Exercise the read-only host operation with a complete, known fixture set.
        import http.client
        import socket
        import time
        (data / "bin/initrd").write_bytes(b"fixture-initrd")
        for name in ("kernel", "initrd"):
            artifact = data / "bin" / ("vmlinux" if name == "kernel" else name)
            valid[name] = {"size": artifact.stat().st_size,
                           "sha256": hashlib.sha256(artifact.read_bytes()).hexdigest()}
        host_helper = data / "bin/browser-vz-engine-supervisor"
        host_helper.write_text("#!/bin/sh\nprintf '%s\\n' '{\"schema\":\"elastos.browser.vm-host-capabilities/v1\",\"available\":true}'\n")
        sidecar.write_text(json.dumps(valid))

        def host_check(name, expected):
            proc = subprocess.run([str(repo / "scripts/browser-vm-artifact-preflight.sh"), "--host-readiness"],
                                  env={**env, "ELASTOS_DEBUGFS_BIN": debugfs},
                                  capture_output=True, text=True, timeout=15, check=True)
            result = json.loads(proc.stdout)
            assert result == {"schema":"elastos.browser.engine-readiness/v1", "readiness":expected}, (name, result)
            checks.append(name)

        host_check("complete fixture host readiness", {"state":"ready"})
        original_kernel = (data / "bin/vmlinux").read_bytes()
        (data / "bin/vmlinux").write_bytes(b"changed kernel")
        host_check("host rejects changed kernel", {"state":"unavailable","reason":"artifact_invalid"})
        (data / "bin/vmlinux").write_bytes(original_kernel)
        (data / "bin/initrd").unlink()
        host_check("host rejects missing initrd", {"state":"unavailable","reason":"preparation_required"})
        (data / "bin/initrd").write_bytes(b"fixture-initrd")
        host_helper.write_text(host_helper.read_text().replace("true", "false"))
        host_check("host rejects unavailable virtualization", {"state":"unavailable","reason":"host_unsupported"})
        host_helper.write_text(host_helper.read_text().replace("false", "true"))

        scripts = data / "scripts"
        scripts.mkdir()
        shutil.copy2(repo / "scripts/browser-vm-artifact-preflight.sh", scripts)
        import shlex
        probe_count = scratch / "readiness-probe-count"
        installed_probe = scripts / "browser-vm-artifact-preflight.sh"
        installed_probe.write_text(installed_probe.read_text().replace("set -euo pipefail",
            "set -euo pipefail\nprintf x >> " + shlex.quote(str(probe_count)), 1))
        control_socket = str(scratch / "readiness.sock")
        service_env = {**env, "ELASTOS_DEBUGFS_BIN":debugfs,
            "ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG":json.dumps({
                "schema":"elastos.browser.vm-control-service.config/v1",
                "control_socket_path":control_socket, "launcher_program":str(host_helper),
                "network_mode":"runtime_net_only", "direct_network":False,
            })}
        service = subprocess.Popen([shutil.which("node"), str(repo / "scripts/browser-vm-control-service.mjs")],
                                   env=service_env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        def get(path):
            client = http.client.HTTPConnection("browser-vm", timeout=12)
            client.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            client.sock.settimeout(12)
            client.sock.connect(control_socket)
            try:
                client.request("GET", path)
                response = client.getresponse()
                assert response.status == 200
                return json.loads(response.read())
            finally:
                client.close()
        try:
            deadline = time.monotonic() + 5
            while not pathlib.Path(control_socket).exists() and time.monotonic() < deadline and service.poll() is None:
                time.sleep(0.02)
            assert pathlib.Path(control_socket).exists(), "control service did not start"
            assert get("/readiness")["readiness"] == {"state":"ready"}
            checks.append("control service reports host readiness")
            assert get("/readiness")["readiness"] == {"state":"ready"}
            assert probe_count.read_text() == "x"
            checks.append("unchanged admitted artifacts reuse verified readiness")
            (data / "bin/vmlinux").write_bytes(b"changed kernel")
            assert get("/readiness")["readiness"] == {"state":"unavailable","reason":"artifact_invalid"}
            checks.append("control service notices artifact change")
            assert probe_count.read_text() == "xx"
            (data / "bin/vmlinux").write_bytes(original_kernel)
            status = get("/status")
            assert status["active_pages"] == status["active_vms"] == status["pending_launches"] == 0
            checks.append("readiness creates no page or VM")
            sleeper_pid = scratch / "readiness-sleeper.pid"
            installed_probe.write_text("#!/bin/sh\necho $$ > " + shlex.quote(str(sleeper_pid)) + "\nexec sleep 30\n")
            started = time.monotonic()
            assert get("/readiness")["readiness"] == {"state":"unavailable","reason":"preparation_required"}
            assert time.monotonic() - started < 10
            try:
                os.kill(int(sleeper_pid.read_text()), 0)
            except ProcessLookupError:
                pass
            else:
                raise AssertionError("timed-out readiness probe remained alive")
            checks.append("slow readiness is bounded and its process is reaped")
        finally:
            service.terminate()
            try:
                _, stderr = service.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                service.kill()
                _, stderr = service.communicate(timeout=5)
            if service.returncode not in (0, -15):
                raise AssertionError(stderr)
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
