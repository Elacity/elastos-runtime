import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const preflight = fileURLToPath(new URL("../browser-vm-artifact-preflight.sh", import.meta.url));
const python = spawnSync("python3", ["-c", "import sys; print(sys.executable)"], { encoding: "utf8" });
assert.equal(python.status, 0, python.stderr);
const pythonPath = python.stdout.trim();
const capabilitySchema = "elastos.browser.vm-host-capabilities/v1";

function identity(file) {
  const bytes = fs.readFileSync(file);
  return { size: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") };
}

function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "browser-native-readiness-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.mkdirSync(path.join(root, "bin"));
  fs.mkdirSync(path.join(root, "scripts"));
  fs.mkdirSync(path.join(root, "browser-vm"));
  const launcher = path.join(root, "bin/browser-vz-engine-supervisor");
  fs.writeFileSync(launcher, `#!/bin/sh
[ "$#" = 1 ] && [ "$1" = --host-capabilities ] || exit 99
printf '%s\\n' "$1" >> "$FIXTURE_PROBE_CALLS"
if [ "$FIXTURE_PROBE_TIMEOUT" = 1 ]; then
  exec "$FIXTURE_REAL_PYTHON" -c 'import time; time.sleep(10)'
fi
printf '%s\\n' "$FIXTURE_PROBE_RESPONSE"
exit "$FIXTURE_PROBE_STATUS"
`, { mode: 0o755 });
  const turn = path.join(root, "bin/turnserver");
  fs.writeFileSync(turn, "#!/bin/sh\nexit 99\n", { mode: 0o755 });
  fs.writeFileSync(path.join(root, "bin/crosvm"), "#!/bin/sh\nexit 99\n", { mode: 0o755 });
  const kernel = path.join(root, "bin/vmlinux");
  const image = path.join(root, "browser-vm/rootfs.ext4");
  const initrd = path.join(root, "browser-vm/initrd");
  const sidecar = path.join(root, "browser-vm/browser-vm-rootfs-manifest.json");
  fs.writeFileSync(kernel, "fixture kernel");
  fs.writeFileSync(image, "fixture image");
  fs.writeFileSync(initrd, "fixture initrd");
  fs.copyFileSync(initrd, path.join(root, "bin/initrd"));
  fs.writeFileSync(sidecar, JSON.stringify({
    schema: "elastos.browser.vm-rootfs-build/v1", ok: true, target_platform: "linux-arm64",
    ...identity(image), kernel: identity(kernel), initrd: identity(initrd),
    preflight: {
      ok: true, missing: [], manifest_errors: [], script_errors: [], audio_default_ready: true,
      required: Object.fromEntries(["manifest", "init", "native_proxy", "runtime_relay", "guest_control_bridge",
        "control_service", "selkies_start", "node", "chromium", "xvfb", "python3", "gst_inspect"]
        .map(name => [name, { ok: true }])),
      optional_audio: Object.fromEntries(["pipewire", "pipewire_pulse", "wireplumber", "pw_cli"]
        .map(name => [name, { ok: true }])),
      manifest: {
        schema: "elastos.browser.vm-target/v1", engine: "chromium_microvm", network_mode: "runtime_net_only",
        direct_network: false, wallet_injection: false, media_transport: "runtime_relay",
        display_mode: "webrtc_remote_display", guarantee_level: "mechanism_microvm",
        runtime_exit_transport: "vsock_relay", display_backend: "vm_selkies_gstreamer_webrtc",
        control_transport: "vsock_relay", control_port: 19092,
      },
    },
  }));
  // The shell invokes this interpreter with its complete production heredoc.
  // Only host identity changes; parsing, admission, I/O and subprocesses stay real.
  const bootstrap = path.join(root, "fixture-python.py");
  fs.writeFileSync(bootstrap, `import os, platform, sys, hashlib
if os.environ.get("FIXTURE_FORBID_HASH") == "1":
    def forbidden_hash(*args, **kwargs):
        raise AssertionError("host readiness attempted to hash image bytes")
    hashlib.sha256 = forbidden_hash
platform.system = lambda: os.environ["FIXTURE_HOST_SYSTEM"]
platform.machine = lambda: os.environ["FIXTURE_HOST_MACHINE"]
sys.argv = sys.argv[1:]
exec(compile(sys.stdin.read(), "<whole-preflight>", "exec"), {"__name__": "__main__"})
`);
  const interpreter = path.join(root, "fixture-python");
  fs.writeFileSync(interpreter, '#!/bin/sh\nexec "$FIXTURE_REAL_PYTHON" "$FIXTURE_BOOTSTRAP" "$@"\n', { mode: 0o755 });
  const legacyCalls = path.join(root, "legacy-calls");
  fs.writeFileSync(path.join(root, "scripts/browser-vm-linux-network.py"),
    `from pathlib import Path\nPath(${JSON.stringify(legacyCalls)}).write_text("retired helper executed")\n`);
  return { root, launcher, turn, legacyCalls, kernel, image, initrd, sidecar, interpreter, bootstrap,
    calls: path.join(root, "native-calls") };
}

function probe(f, {
  platform = "linux-arm64", hostSystem = "Linux", hostMachine = "aarch64",
  launcher = f.launcher, turn = f.turn,
  response = { schema: capabilitySchema, available: true }, status = 0, timeout = false,
  args = ["--host-readiness"], expectedStatus = 0, envOverrides = {},
} = {}) {
  const env = { PATH: process.env.PATH, ELASTOS_BROWSER_VM_PLATFORM: platform, ELASTOS_BROWSER_VM_DATA_DIR: f.root,
    ELASTOS_BROWSER_VM_VZ_SUPERVISOR: launcher, ELASTOS_BROWSER_VM_TURN_PROGRAM: turn,
    ELASTOS_BROWSER_VM_PYTHON: f.interpreter, ELASTOS_DEBUGFS_BIN: path.join(f.root, "absent-debugfs"),
    FIXTURE_REAL_PYTHON: pythonPath, FIXTURE_BOOTSTRAP: f.bootstrap,
    FIXTURE_HOST_SYSTEM: hostSystem, FIXTURE_HOST_MACHINE: hostMachine,
    FIXTURE_PROBE_CALLS: f.calls, FIXTURE_PROBE_RESPONSE: typeof response === "string" ? response : JSON.stringify(response),
    FIXTURE_PROBE_STATUS: String(status), FIXTURE_PROBE_TIMEOUT: timeout ? "1" : "0", ...envOverrides };
  const child = spawnSync("bash", [preflight, ...args], { env, encoding: "utf8", timeout: 5000 });
  assert.equal(child.status, expectedStatus, child.stderr || String(child.error));
  assert.equal(fs.existsSync(f.legacyCalls), false, "retired network helper stays unused");
  if (expectedStatus === 2) {
    assert.match(child.stderr, /usage:/);
    return;
  }
  return JSON.parse(child.stdout);
}

export { fixture, probe };
