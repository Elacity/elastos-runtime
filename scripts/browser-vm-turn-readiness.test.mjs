import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
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

// The probe mock below always reports ready (unless the TURN or host checks
// refuse), so the persisted-stamp tests prove cache mechanics, not image admission.
function controller(t, { linux = false, delayed = false, from = null, arch = process.arch, foreignOwner = false,
  now = null } = {}) {
  // `from` restarts the service on an existing fixture: fresh memory, same disk.
  const f = from ? { root: from.root, launcher: from.launcher, turn: from.turn, env: from.env } : fixture(t);
  if (linux) {
    f.env.ELASTOS_BROWSER_VM_PLATFORM = "linux-amd64";
    delete f.env[turnKey];
  }
  const probes = [], stats = [], networkChecks = [], messages = [], events = [];
  const contextFs = Object.assign(Object.create(fs), {
    statSync(file, options) {
      stats.push(file);
      return fs.statSync(file === "/dev/kvm" ? path.join(f.root, "kvm") : file, options);
    },
    fstatSync(fd, options) {
      const info = fs.fstatSync(fd, options);
      return foreignOwner ? Object.assign(Object.create(Object.getPrototypeOf(info)), info, { uid: info.uid + 1 }) : info;
    },
  });
  const context = vm.createContext({ path, os, Buffer, fs: contextFs,
    ...(now === null ? {} : { Date: { now: () => now } }),
    process: { env: f.env, platform: linux ? "linux" : "darwin", arch, pid: process.pid, getuid: process.getuid,
      stderr: { write: message => messages.push(message) } },
    logEvent: (event, fields) => events.push({ event, ...fields }),
    execFileSync(command, args) {
      assert.equal(command, "python3");
      assert.deepEqual(Array.from(args), [path.join(f.root, "scripts/browser-vm-linux-network.py"), "check"]);
      networkChecks.push(args);
      if (f.env.fixtureNetworkMissing) throw { stderr: Buffer.from("As root, run: python3 /fixture/browser-vm-linux-network.py setup --user fixture") };
    },
    execFile(script, args, options, done) {
      assert.equal(script, path.join(f.root, "scripts/browser-vm-artifact-preflight.sh"));
      assert.equal(args.join(), "--host-readiness");
      const result = linux ? ready : probe(options.env);
      const finish = () => done(null, JSON.stringify(result));
      probes.push({ finish });
      if (!delayed) finish();
    },
  });
  const api = vm.runInContext(`${readiness}\n({engineReadiness})`, context);
  return { ...f, probes, stats, networkChecks, messages, events, cache: path.join(f.root, "browser-vm/host-readiness.json"),
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

test("cache mechanics (mock probe): a restarted service reuses persisted stamps without probing", async t => {
  const first = controller(t);
  assert.deepEqual(await first.read(), ready);
  assert.equal(first.probes.length, 1);
  const restarted = controller(t, { from: first });
  assert.deepEqual(await restarted.read(), ready);
  assert.equal(restarted.probes.length, 0);
  assert.deepEqual(restarted.events.map(e => e.source), ["persisted_stamps"]);
  const info = fs.lstatSync(first.cache);
  assert.equal(info.mode & 0o777, 0o600);
  const record = JSON.parse(fs.readFileSync(first.cache, "utf8"));
  assert.deepEqual(Object.keys(record).sort(), ["input_stamps", "probe_result", "schema"]);
  assert.deepEqual(fs.readdirSync(path.dirname(first.cache)).filter(name => name.endsWith(".tmp")), []);
});

for (const change of ["rewrite-rootfs", "touch-manifest", "replace-preflight", "corrupt-cache", "unready-cache"]) {
  test(`cache mechanics (mock probe): persisted stamps are not trusted after ${change}`, async t => {
    const first = controller(t);
    assert.deepEqual(await first.read(), ready);
    if (change === "rewrite-rootfs") fs.writeFileSync(path.join(first.root, "browser-vm/rootfs.ext4"), "rewritten rootfs");
    if (change === "touch-manifest") {
      const later = new Date(Date.now() + 5000);
      fs.utimesSync(path.join(first.root, "browser-vm/browser-vm-rootfs-manifest.json"), later, later);
    }
    if (change === "replace-preflight") {
      const script = path.join(first.root, "scripts/browser-vm-artifact-preflight.sh");
      fs.writeFileSync(`${script}.new`, fs.readFileSync(script));
      fs.renameSync(`${script}.new`, script); // Same bytes, new inode.
    }
    if (change === "corrupt-cache") fs.writeFileSync(first.cache, "{not json");
    if (change === "unready-cache") {
      const record = JSON.parse(fs.readFileSync(first.cache, "utf8"));
      record.probe_result = unavailable();
      fs.writeFileSync(first.cache, JSON.stringify(record));
    }
    const restarted = controller(t, { from: first });
    assert.deepEqual(await restarted.read(), ready);
    assert.equal(restarted.probes.length, 1, "a changed input or cache must probe again");
  });
}

test("a platform switch on the same Darwin host re-probes in memory and after restart", async t => {
  const first = controller(t);
  assert.deepEqual(await first.read(), ready);
  first.env.ELASTOS_BROWSER_VM_PLATFORM = "darwin-amd64";
  assert.deepEqual(await first.read(), unavailable("host_unsupported"));
  assert.equal(first.probes.length, 2);
  const restarted = controller(t, { from: first });
  assert.deepEqual(await restarted.read(), unavailable("host_unsupported"));
  assert.equal(restarted.probes.length, 1, "an arm64 ready record must not admit darwin-amd64");
});

test("cache mechanics (mock probe): another host architecture does not reuse persisted stamps", async t => {
  const first = controller(t, { arch: "arm64" });
  assert.deepEqual(await first.read(), ready);
  const restarted = controller(t, { from: first, arch: "x64" });
  assert.deepEqual(await restarted.read(), ready);
  assert.equal(restarted.probes.length, 1);
});

for (const unsafe of ["symlink", "group-readable", "hard-link", "foreign-owner", "oversize", "directory"]) {
  test(`cache mechanics (mock probe): unsafe persisted state (${unsafe}) is ignored and re-probed`, async t => {
    const first = controller(t);
    assert.deepEqual(await first.read(), ready);
    const record = fs.readFileSync(first.cache, "utf8");
    if (unsafe === "symlink") {
      const elsewhere = path.join(first.root, "elsewhere.json");
      fs.writeFileSync(elsewhere, record, { mode: 0o600 });
      fs.unlinkSync(first.cache);
      fs.symlinkSync(elsewhere, first.cache);
    }
    if (unsafe === "group-readable") fs.chmodSync(first.cache, 0o640);
    if (unsafe === "hard-link") fs.linkSync(first.cache, path.join(first.root, "second-name.json"));
    if (unsafe === "oversize") fs.writeFileSync(first.cache, record + " ".repeat(70 * 1024));
    if (unsafe === "directory") {
      fs.unlinkSync(first.cache);
      fs.mkdirSync(first.cache, { mode: 0o700 });
    }
    const restarted = controller(t, { from: first, foreignOwner: unsafe === "foreign-owner" });
    assert.deepEqual(await restarted.read(), ready);
    assert.equal(restarted.probes.length, 1, "unsafe state must never be trusted");
    if (unsafe === "symlink") assert.equal(fs.readFileSync(path.join(first.root, "elsewhere.json"), "utf8"), record,
      "a write must replace the link, not follow it");
  });
}

test("cache mechanics (mock probe): an unsafe parent directory is neither read nor written", async t => {
  const first = controller(t);
  assert.deepEqual(await first.read(), ready);
  const parent = path.dirname(first.cache);
  fs.chmodSync(parent, 0o777);
  try {
    const before = fs.readFileSync(first.cache, "utf8");
    fs.writeFileSync(path.join(first.root, "bin/vmlinux"), "changed kernel identity");
    const restarted = controller(t, { from: first });
    assert.deepEqual(await restarted.read(), ready);
    assert.equal(restarted.probes.length, 1);
    assert.equal(fs.readFileSync(first.cache, "utf8"), before, "no write into a shared parent");
  } finally {
    fs.chmodSync(parent, 0o755);
  }
});

test("cache mechanics (mock probe): a planted temporary name is neither followed nor removed", async t => {
  const c = controller(t, { now: 4242 });
  const victim = path.join(c.root, "victim");
  fs.writeFileSync(victim, "victim bytes");
  const planted = `${c.cache}.${process.pid}.4242.tmp`;
  fs.symlinkSync(victim, planted);
  assert.deepEqual(await c.read(), ready);
  assert.equal(fs.readFileSync(victim, "utf8"), "victim bytes");
  assert.ok(fs.lstatSync(planted).isSymbolicLink(), "a temporary name this call did not create stays");
  assert.equal(fs.existsSync(c.cache), false, "no stamps are published without an owned temporary file");
});

test("service start and the first open share one readiness probe", async t => {
  const c = controller(t, { delayed: true });
  const prewarm = c.read();
  const open = c.read();
  assert.equal(c.probes.length, 1);
  c.probes[0].finish();
  assert.deepEqual(await prewarm, ready);
  assert.deepEqual(await open, ready);
  assert.deepEqual(await c.read(), ready);
  assert.equal(c.probes.length, 1);
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

test("Linux readiness retains KVM/crosvm identity and does not require VZ TURN", async t => {
  const c = controller(t, { linux: true });
  assert.deepEqual(await c.read(), ready);
  c.env[turnKey] = path.join(c.root, "absent");
  assert.deepEqual(await c.read(), ready);
  assert.equal(c.probes.length, 1);
  assert.equal(c.networkChecks.length, 2);
  assert.ok(c.stats.includes("/dev/kvm"));
  assert.ok(c.stats.includes(path.join(c.root, "bin/crosvm")));
});

test("Linux cached image readiness refuses missing network setup with its repair command", async t => {
  const c = controller(t, { linux: true });
  assert.deepEqual(await c.read(), ready);
  c.env.fixtureNetworkMissing = "1";
  const result = await c.read();
  assert.equal(result.readiness.state, "unavailable");
  assert.equal(result.readiness.reason, "preparation_required");
  assert.match(c.messages[0], /As root, run: python3 .* setup --user fixture/);
  assert.deepEqual(result, unavailable(), "host repair paths stay outside the strict Engine contract");
  assert.equal(c.probes.length, 1);
});

test("remote VZ readiness remains unsupported without a local probe", async t => {
  const c = controller(t);
  delete c.env[turnKey];
  assert.deepEqual(await c.read("/fixture/browser-vm-remote-vz-launcher"), unavailable("readiness_unsupported"));
  assert.equal(c.probes.length, 0);
  assert.equal(c.stats.length, 0);
});
