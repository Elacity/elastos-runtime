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
const readiness = control.slice(control.indexOf("let verifiedHostReadiness ="),
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

function probe(env, imageReason = null) {
  // Execute the actual host-readiness branch with real executable-file checks.
  // Image hashing is covered by the artifact smoke; host eligibility is a fixture.
  const program = `import json, os, pathlib, platform as host_platform, subprocess, sys
host_platform.system = lambda: "Darwin"
host_platform.machine = lambda: "arm64"
platform = os.environ["ELASTOS_BROWSER_VM_PLATFORM"]
mode = "--host-readiness"
vz_supervisor = os.environ["ELASTOS_BROWSER_VM_VZ_SUPERVISOR"]
verify_image_set = lambda: json.loads(sys.argv[1])
local_substrate_artifacts_ready = True
${hostReadiness}`;
  const child = spawnSync("python3", ["-c", program, JSON.stringify(imageReason)], {
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

test("TURN readiness does not replace image integrity or host compatibility checks", t => {
  const f = fixture(t);
  assert.deepEqual(probe(f.env, "artifact_invalid"), unavailable("artifact_invalid"));
  delete f.env[turnKey];
  assert.deepEqual(probe(f.env, "artifact_invalid"), unavailable("artifact_invalid"));
  f.env.ELASTOS_BROWSER_VM_PLATFORM = "linux-amd64";
  assert.deepEqual(probe(f.env), unavailable("host_unsupported"));
});

function controller(t, { linux = false, delayed = false } = {}) {
  const f = fixture(t);
  if (linux) {
    f.env.ELASTOS_BROWSER_VM_PLATFORM = "linux-amd64";
    delete f.env[turnKey];
  }
  const probes = [], stats = [];
  const context = vm.createContext({ path, process: { env: f.env, platform: linux ? "linux" : "darwin" },
    fs: { statSync(file, options) {
      stats.push(file);
      return fs.statSync(file === "/dev/kvm" ? path.join(f.root, "kvm") : file, options);
    } },
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
  return { ...f, probes, stats,
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
  assert.ok(c.stats.includes("/dev/kvm"));
  assert.ok(c.stats.includes(path.join(c.root, "bin/crosvm")));
});

test("remote VZ readiness remains unsupported without a local probe", async t => {
  const c = controller(t);
  delete c.env[turnKey];
  assert.deepEqual(await c.read("/fixture/browser-vm-remote-vz-launcher"), unavailable("readiness_unsupported"));
  assert.equal(c.probes.length, 0);
  assert.equal(c.stats.length, 0);
});
