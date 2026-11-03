import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";

const preflight = fs.readFileSync(new URL("./browser-vm-artifact-preflight.sh", import.meta.url), "utf8");
const hostReadiness = preflight.slice(preflight.indexOf('if mode == "--host-readiness":'),
  preflight.indexOf("\nif launch_ready:"));
const control = fs.readFileSync(new URL("./browser-vm-control-service.mjs", import.meta.url), "utf8");
const readiness = control.slice(control.indexOf("let readyHostReadiness ="),
  control.indexOf("const OPEN_REQUEST_ENV ="));
const ready = { schema: "elastos.browser.engine-readiness/v1", readiness: { state: "ready" } };
const unavailable = (reason = "preparation_required") => ({ ...ready, readiness: { state: "unavailable", reason } });
const turnKey = "ELASTOS_BROWSER_VM_TURN_PROGRAM";

function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "browser-turn-readiness-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  for (const directory of ["bin", "browser-vm", "scripts"]) fs.mkdirSync(path.join(root, directory));
  for (const file of ["bin/vmlinux", "bin/initrd", "browser-vm/initrd", "browser-vm/rootfs.ext4",
    "browser-vm/browser-vm-rootfs-manifest.json", "scripts/browser-vm-artifact-preflight.sh", "bin/crosvm", "kvm"]) {
    fs.writeFileSync(path.join(root, file), "disposable readiness identity fixture");
  }
  const launcher = path.join(root, "bin/browser-vz-engine-supervisor");
  fs.writeFileSync(launcher, `#!/bin/sh\n[ "$1" = --host-capabilities ] || exit 99\nprintf '%s\\n' '{"schema":"elastos.browser.vm-host-capabilities/v1","available":true}'\n`, { mode: 0o755 });
  const turn = path.join(root, "bin/turnserver");
  // Readiness inspects this file; executing it would fail the test.
  fs.writeFileSync(turn, "#!/bin/sh\nexit 99\n", { mode: 0o755 });
  const env = { ELASTOS_BROWSER_VM_PLATFORM: "darwin-arm64", ELASTOS_BROWSER_VM_DATA_DIR: root,
    ELASTOS_BROWSER_VM_VZ_SUPERVISOR: launcher, [turnKey]: turn };
  return { root, launcher, turn, env };
}

function probe(env, imageReason = null, hostProbe = "ready") {
  // Execute the actual host-readiness branch with real executable-file checks.
  // Image hashing and the supervisor process are covered by the artifact smoke.
  // Stub host eligibility so suite load cannot exhaust its two-second deadline.
  const program = `import json, os, pathlib, platform as host_platform, subprocess, sys
from types import SimpleNamespace
host_platform.system = lambda: "Darwin"
host_platform.machine = lambda: "arm64"
platform = os.environ["ELASTOS_BROWSER_VM_PLATFORM"]
mode = "--host-readiness"
vz_supervisor = os.environ["ELASTOS_BROWSER_VM_VZ_SUPERVISOR"]
def host_probe(command, **options):
    assert command == [vz_supervisor, "--host-capabilities"]
    assert options == dict(capture_output=True, text=True, timeout=2, check=True)
    if sys.argv[2] == "timeout":
        raise subprocess.TimeoutExpired(command, options["timeout"])
    return SimpleNamespace(stdout=json.dumps({"schema": "elastos.browser.vm-host-capabilities/v1", "available": True}))
subprocess.run = host_probe
verify_image_set = lambda: json.loads(sys.argv[1])
local_substrate_artifacts_ready = True
${hostReadiness}`;
  const child = spawnSync("python3", ["-c", program, JSON.stringify(imageReason), hostProbe], {
    env: { PATH: process.env.PATH, ...env }, encoding: "utf8", timeout: 5000,
  });
  assert.equal(child.status, 0, child.stderr || String(child.error));
  return JSON.parse(child.stdout);
}

for (const kind of ["valid", "missing", "empty", "nonexistent", "relative", "directory", "nonexecutable", "symlink", "dangling-symlink"]) {
  test(`Darwin host readiness checks configured TURN: ${kind}`, t => {
    const f = fixture(t);
    if (kind === "missing") delete f.env[turnKey];
    if (kind === "empty") f.env[turnKey] = "";
    if (kind === "nonexistent") f.env[turnKey] = path.join(f.root, "absent-turnserver");
    if (kind === "relative") f.env[turnKey] = path.relative(process.cwd(), f.turn);
    if (kind === "directory") f.env[turnKey] = path.dirname(f.turn);
    if (kind === "nonexecutable") fs.chmodSync(f.turn, 0o644);
    if (kind.endsWith("symlink")) {
      f.env[turnKey] = path.join(f.root, "turn-link");
      fs.symlinkSync(kind === "symlink" ? f.turn : path.join(f.root, "absent"), f.env[turnKey]);
    }
    assert.deepEqual(probe(f.env), ["valid", "symlink"].includes(kind) ? ready : unavailable());
  });
}

test("a supervisor timeout keeps host readiness unavailable", t => {
  const f = fixture(t);
  assert.deepEqual(probe(f.env, null, "timeout"), unavailable("readiness_unsupported"));
});

test("TURN readiness does not replace image integrity or host compatibility checks", t => {
  const f = fixture(t);
  assert.deepEqual(probe(f.env, "artifact_invalid"), unavailable("artifact_invalid"));
  delete f.env[turnKey];
  assert.deepEqual(probe(f.env, "artifact_invalid"), unavailable("artifact_invalid"));
  f.env.ELASTOS_BROWSER_VM_PLATFORM = "linux-amd64";
  assert.deepEqual(probe(f.env), unavailable("host_unsupported"));
});

// The probe mock below reports ready unless the TURN or host checks refuse, so
// the cache tests prove probe sharing and keying mechanics, not image admission.
function controller(t, { platform = "darwin-arm64", delayed = false } = {}) {
  const linux = platform.startsWith("linux-");
  const f = fixture(t);
  f.env.ELASTOS_BROWSER_VM_PLATFORM = platform;
  const probes = [], stats = [], capabilityChecks = [], messages = [], events = [];
  const proc = { env: f.env, platform: linux ? "linux" : "darwin", arch: process.arch,
    stderr: { write: message => messages.push(message) } };
  const context = vm.createContext({ path, os, process: proc,
    logEvent: (event, fields) => events.push({ event, ...fields }),
    fs: { statSync(file, options) {
      stats.push(file);
      return fs.statSync(file === "/dev/kvm" ? path.join(f.root, "kvm") : file, options);
    }, accessSync: fs.accessSync, constants: fs.constants },
    execFileSync(command, args, options) {
      assert.equal(command, f.launcher);
      assert.deepEqual(Array.from(args), ["--host-capabilities"]);
      assert.ok(options.timeout > 0 && options.timeout <= 5000);
      capabilityChecks.push(args);
      if (f.env.fixtureHostFailure) throw { stderr: Buffer.from("selected Linux transport requires repair") };
      return f.env.fixtureHostCapabilities || JSON.stringify({
        schema: "elastos.browser.vm-host-capabilities/v1", available: true,
      });
    },
    execFile(script, args, options, done) {
      assert.equal(script, path.join(f.root, "scripts/browser-vm-artifact-preflight.sh"));
      assert.equal(args.join(), "--host-readiness");
      assert.equal(options.env.ELASTOS_BROWSER_VM_VZ_SUPERVISOR, f.launcher);
      const turn = options.env[turnKey];
      const result = linux ? (turn && fs.existsSync(turn) ? ready : unavailable()) : probe(options.env);
      const finish = () => done(null, JSON.stringify(result));
      probes.push({ finish, options });
      if (!delayed) finish();
    },
  });
  const api = vm.runInContext(`${readiness}\n({engineReadiness})`, context);
  return { ...f, proc, probes, stats, capabilityChecks, messages, events,
    read: async (launcher = f.launcher) => JSON.parse(JSON.stringify(await api.engineReadiness({ launcher_program: launcher }))),
  };
}

test("unchanged TURN and image identities reuse successful readiness", async t => {
  const c = controller(t);
  assert.deepEqual(await c.read(), ready);
  assert.deepEqual(await c.read(), ready);
  assert.equal(c.probes.length, 1);
  fs.writeFileSync(path.join(c.root, "bin/vmlinux"), "changed kernel identity");
  assert.deepEqual(await c.read(), ready);
  assert.equal(c.probes.length, 2);
});

test("mechanics (mock probe): service start and the first open share one readiness probe", async t => {
  const c = controller(t, { delayed: true });
  const prewarm = c.read();
  const open = c.read();
  assert.equal(c.probes.length, 1);
  c.probes[0].finish();
  assert.deepEqual(await prewarm, ready);
  assert.deepEqual(await open, ready);
  assert.deepEqual(await c.read(), ready);
  assert.equal(c.probes.length, 1);
  assert.deepEqual(c.events.map(e => [e.event, e.source, e.state]), [["host_readiness", "probe", "ready"]]);
});

test("a platform switch on the same Darwin host re-probes and stays unsupported", async t => {
  const c = controller(t);
  assert.deepEqual(await c.read(), ready);
  c.env.ELASTOS_BROWSER_VM_PLATFORM = "darwin-amd64";
  assert.deepEqual(await c.read(), unavailable("host_unsupported"), "an arm64 ready result must not admit darwin-amd64");
  assert.deepEqual(await c.read(), unavailable("host_unsupported"));
  assert.equal(c.probes.length, 3, "an unsupported result is never cached");
});

test("mechanics (mock probe): a host architecture change re-probes", async t => {
  const c = controller(t);
  assert.deepEqual(await c.read(), ready);
  c.proc.arch = c.proc.arch === "arm64" ? "x64" : "arm64";
  assert.deepEqual(await c.read(), ready);
  assert.equal(c.probes.length, 2);
});

for (const tool of ["debugfs", "python3"]) {
  test(`mechanics (mock probe): replacing ${tool} found through PATH re-probes`, async t => {
    const c = controller(t);
    const tools = path.join(c.root, "tools");
    fs.mkdirSync(tools);
    const real = spawnSync("sh", ["-c", "command -v python3"], { encoding: "utf8" }).stdout.trim();
    const write = () => {
      const next = path.join(tools, `${tool}.new`);
      fs.writeFileSync(next, tool === "python3" ? `#!/bin/sh\nexec "${real}" "$@"\n` : "#!/bin/sh\nexit 0\n", { mode: 0o755 });
      fs.renameSync(next, path.join(tools, tool)); // Same name on PATH, new inode.
    };
    write();
    c.env.PATH = `${tools}:${process.env.PATH}`;
    assert.deepEqual(await c.read(), ready);
    assert.deepEqual(await c.read(), ready);
    assert.equal(c.probes.length, 1);
    write();
    assert.deepEqual(await c.read(), ready);
    assert.equal(c.probes.length, 2, `a new ${tool} on the same PATH must not reuse readiness`);
  });
}

test("mechanics (mock probe): an empty PATH entry resolves debugfs in the current directory", async t => {
  const c = controller(t);
  const cwd = process.cwd();
  const here = path.join(c.root, "cwd");
  fs.mkdirSync(here);
  const write = () => {
    fs.writeFileSync(path.join(here, "debugfs.new"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
    fs.renameSync(path.join(here, "debugfs.new"), path.join(here, "debugfs"));
  };
  write();
  c.env.PATH = `:${process.env.PATH}`;
  process.chdir(here);
  try {
    assert.deepEqual(await c.read(), ready);
    write();
    assert.deepEqual(await c.read(), ready);
    assert.equal(c.probes.length, 2, "the shell runs ./debugfs for an empty PATH entry");
  } finally {
    process.chdir(cwd);
  }
});

test("mechanics (mock probe): a PATH file this user cannot execute is not the resolved debugfs", async t => {
  const c = controller(t);
  const tools = path.join(c.root, "tools");
  fs.mkdirSync(tools);
  const write = () => {
    // Group/other execute bits only: the shell skips it for this user.
    fs.writeFileSync(path.join(tools, "debugfs.new"), "#!/bin/sh\nexit 0\n");
    fs.chmodSync(path.join(tools, "debugfs.new"), 0o611);
    fs.renameSync(path.join(tools, "debugfs.new"), path.join(tools, "debugfs"));
  };
  write();
  c.env.PATH = `${tools}:${process.env.PATH}`;
  assert.deepEqual(await c.read(), ready);
  write();
  assert.deepEqual(await c.read(), ready);
  assert.equal(c.probes.length, 1, "replacing a file the preflight would not run is not a new input");
});

test("mechanics (mock probe): a configured but absent debugfs keeps a successful probe ready", async t => {
  const c = controller(t);
  c.env.ELASTOS_DEBUGFS_BIN = path.join(c.root, "absent-debugfs");
  assert.deepEqual(await c.read(), ready, "optional debugfs absence must not turn ready into preparation_required");
  assert.deepEqual(await c.read(), ready);
  assert.equal(c.probes.length, 1);
  fs.writeFileSync(c.env.ELASTOS_DEBUGFS_BIN, "#!/bin/sh\nexit 0\n", { mode: 0o755 });
  assert.deepEqual(await c.read(), ready);
  assert.equal(c.probes.length, 2, "a debugfs that appears is a new input");
});

test("the control service probes readiness when it starts, before any request", async t => {
  const f = fixture(t);
  const child = spawn(process.execPath, [new URL("./browser-vm-control-service.mjs", import.meta.url).pathname], {
    env: { ...f.env, PATH: process.env.PATH, ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG: JSON.stringify({
      schema: "elastos.browser.vm-control-service.config/v1", control_socket_path: path.join(f.root, "c.sock"),
      launcher_program: f.launcher, launcher_args: [] }) },
    stdio: ["ignore", "ignore", "pipe"],
  });
  const closed = new Promise(resolve => child.once("close", resolve));
  let stderr = "";
  try {
    const event = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`no startup probe: ${stderr}`)), 5000);
      child.stderr.on("data", chunk => {
        stderr += chunk;
        const line = stderr.split("\n").find(entry => entry.includes('"host_readiness"'));
        if (line) { clearTimeout(timer); resolve(JSON.parse(line)); }
      });
    });
    // The fixture preflight is not executable, so the eager probe reports it.
    assert.equal(event.source, "probe");
    assert.equal(event.reason, "preparation_required");
  } finally {
    // Stop the service before the fixture directory is removed.
    child.kill("SIGKILL");
    await closed;
  }
});

for (const kind of ["unset", "different-path", "remove", "replace", "chmod", "retarget-symlink"]) {
  test(`cached readiness observes TURN ${kind}`, async t => {
    const c = controller(t);
    if (kind === "retarget-symlink") {
      const link = path.join(c.root, "turn-link");
      fs.symlinkSync(c.turn, link);
      c.env[turnKey] = link;
    }
    assert.deepEqual(await c.read(), ready);
    if (kind === "unset") delete c.env[turnKey];
    if (kind === "different-path") {
      const alias = path.join(c.root, "turn-alias");
      fs.linkSync(c.turn, alias); // Same inode and bytes, different configured path.
      c.env[turnKey] = alias;
    }
    if (kind === "remove") fs.unlinkSync(c.turn);
    if (kind === "replace") {
      const replacement = path.join(c.root, "replacement");
      fs.writeFileSync(replacement, fs.readFileSync(c.turn), { mode: 0o755 });
      fs.renameSync(replacement, c.turn); // Same path/bytes, new inode.
    }
    if (kind === "chmod") fs.chmodSync(c.turn, 0o644);
    if (kind === "retarget-symlink") {
      fs.unlinkSync(c.env[turnKey]);
      fs.symlinkSync(path.join(c.root, "absent"), c.env[turnKey]);
    }
    const expected = ["different-path", "replace"].includes(kind) ? ready : unavailable();
    assert.deepEqual(await c.read(), expected);
    assert.equal(c.probes.length, 2, "changed TURN identity must not reuse cached ready");
  });
}

test("TURN removal during a successful probe prevents publishing or caching ready", async t => {
  const c = controller(t, { delayed: true });
  const pending = c.read();
  fs.unlinkSync(c.turn);
  c.probes[0].finish();
  assert.deepEqual(await pending, unavailable());
  fs.writeFileSync(c.turn, "#!/bin/sh\nexit 99\n", { mode: 0o755 });
  const next = c.read();
  c.probes[1].finish();
  assert.deepEqual(await next, ready);
  assert.deepEqual(await c.read(), ready);
  assert.equal(c.probes.length, 2);
});

test("ARM Linux readiness uses its native capability contract before cached image readiness", async t => {
  const c = controller(t, { platform: "linux-arm64" });
  assert.deepEqual(await c.read(), ready);
  assert.deepEqual(await c.read(), ready);
  assert.equal(c.probes.length, 1);
  assert.equal(c.capabilityChecks.length, 2);
  assert.ok(c.stats.includes("/dev/kvm"));
  assert.ok(c.stats.includes(path.join(c.root, "bin/crosvm")));
  assert.ok(c.stats.includes(path.join(c.root, "bin/initrd")));
  assert.ok(c.stats.includes(c.turn));
  delete c.env[turnKey];
  assert.deepEqual(await c.read(), unavailable());
  assert.equal(c.probes.length, 2);
});

test("ARM Linux readiness survives a VM exit's /dev/kvm time change but not a device change", async t => {
  const c = controller(t, { platform: "linux-arm64" });
  const kvm = path.join(c.root, "kvm");
  assert.deepEqual(await c.read(), ready);
  // Every VM exit updates /dev/kvm's mtime and ctime (#90, Jetson).
  const later = new Date(Date.now() + 60_000);
  fs.utimesSync(kvm, later, later);
  assert.deepEqual(await c.read(), ready);
  assert.equal(c.probes.length, 1, "a VM exit must not re-hash the image");
  fs.chmodSync(kvm, 0o600);
  assert.deepEqual(await c.read(), ready);
  assert.equal(c.probes.length, 2, "changed device permissions probe again");
});

test("a readiness probe has room for a first-time image hash on slow storage", async t => {
  const c = controller(t, { platform: "linux-arm64" });
  assert.deepEqual(await c.read(), ready);
  // 82 s measured cold on a Jetson SD card; the old 30 s limit refused it.
  assert.equal(c.probes[0].options.timeout, 240_000);
});

for (const capabilities of ["failure", "{", '{"available":true}',
  '{"schema":"elastos.browser.vm-host-capabilities/v1","available":false}']) {
  test(`ARM Linux cached readiness observes native capability loss: ${capabilities}`, async t => {
    const c = controller(t, { platform: "linux-arm64" });
    assert.deepEqual(await c.read(), ready);
    if (capabilities === "failure") c.env.fixtureHostFailure = "1";
    else c.env.fixtureHostCapabilities = capabilities;
    assert.deepEqual(await c.read(), unavailable());
    assert.equal(c.probes.length, 1);
    assert.equal(c.capabilityChecks.length, 2);
  });
}

test("ARM Linux readiness rejects retired and unowned local launchers before probing", async t => {
  const c = controller(t, { platform: "linux-arm64" });
  for (const launcher of [path.join(c.root, "bin/browser-vm-local-crosvm-launcher"),
    "/other-home/bin/browser-vz-engine-supervisor"]) {
    assert.deepEqual(await c.read(launcher), unavailable("readiness_unsupported"));
  }
  assert.equal(c.probes.length, 0);
  assert.equal(c.capabilityChecks.length, 0);
});

test("Linux amd64 readiness stays hidden on the ARM qualification candidate", async t => {
  const c = controller(t, { platform: "linux-amd64" });
  assert.deepEqual(await c.read(), unavailable("host_unsupported"));
  assert.equal(c.probes.length, 0);
  assert.equal(c.capabilityChecks.length, 0);
});

test("remote VZ readiness remains unsupported without a local probe", async t => {
  const c = controller(t);
  delete c.env[turnKey];
  assert.deepEqual(await c.read("/fixture/browser-vm-remote-vz-launcher"), unavailable("readiness_unsupported"));
  assert.equal(c.probes.length, 0);
  assert.equal(c.stats.length, 0);
});
