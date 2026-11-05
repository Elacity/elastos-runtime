import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { fixture, probe } from "./lib/browser-vm-readiness-fixture.mjs";

const generator = fileURLToPath(new URL("./browser-source-home-config.mjs", import.meta.url));
const capabilitySchema = "elastos.browser.vm-host-capabilities/v1";
const ready = { schema: "elastos.browser.engine-readiness/v1", readiness: { state: "ready" } };
const unavailable = reason => ({ ...ready, readiness: { state: "unavailable", reason } });

for (const transport of ["kernel-vhost-vsock", "userspace-vhost-user-vsock"]) {
  test(`Linux ARM64 readiness uses the native ${transport} capability`, t => {
    const f = fixture(t);
    assert.deepEqual(probe(f, { response: { schema: capabilitySchema, available: true, transport } }), ready);
    assert.equal(fs.readFileSync(f.calls, "utf8"), "--host-capabilities\n");
  });
}

test("Darwin ARM64 keeps the same native capability contract", t => {
  const f = fixture(t);
  assert.deepEqual(probe(f, { platform: "darwin-arm64", hostSystem: "Darwin", hostMachine: "arm64" }), ready);
  assert.equal(fs.readFileSync(f.calls, "utf8"), "--host-capabilities\n");
});

test("generated native Linux release configuration passes the whole preflight", t => {
  const f = fixture(t), imageSet = path.join(f.root, "browser-vm/image-set");
  fs.mkdirSync(imageSet);
  for (const [source, name] of [[f.image, "rootfs.ext4"], [f.sidecar, "browser-vm-rootfs-manifest.json"],
    [f.kernel, "vmlinux"], [f.initrd, "initrd"]]) {
    fs.renameSync(source, path.join(imageSet, name));
  }
  fs.unlinkSync(path.join(f.root, "bin/initrd"));
  const generated = spawnSync(process.execPath, [generator, "--data-dir", f.root,
    "--platform", "linux-arm64", "--release-image"], { encoding: "utf8", timeout: 5000,
    env: { ELASTOS_BROWSER_VM_TURN_PROGRAM: f.turn, ELASTOS_BROWSER_VM_PYTHON: f.interpreter,
      ELASTOS_DEBUGFS_BIN: path.join(f.root, "absent-debugfs") } });
  assert.equal(generated.status, 0, generated.stderr || String(generated.error));
  const config = JSON.parse(fs.readFileSync(path.join(f.root, "config/browser-engine-adapter.json")));
  const env = config.adapters[0].supervisor.env;
  assert.equal(env.ELASTOS_BROWSER_VM_INITRAMFS, path.join(imageSet, "initrd"));
  assert.equal(env.ELASTOS_BROWSER_VM_INITRD, undefined);
  assert.deepEqual(probe(f, { envOverrides: env }), ready);
  assert.equal(fs.readFileSync(f.calls, "utf8"), "--host-capabilities\n");
});

for (const kind of ["missing", "relative", "directory", "nonexecutable"]) {
  test(`native launcher requires an absolute executable file: ${kind}`, t => {
    const f = fixture(t);
    let launcher = f.launcher;
    if (kind === "missing") fs.unlinkSync(launcher);
    if (kind === "relative") launcher = path.relative(process.cwd(), launcher);
    if (kind === "directory") launcher = path.dirname(launcher);
    if (kind === "nonexecutable") fs.chmodSync(launcher, 0o644);
    assert.deepEqual(probe(f, { launcher }), unavailable("preparation_required"));
    assert.equal(fs.existsSync(f.calls), false);
  });
}

for (const name of ["browser-vm-local-crosvm-launcher.mjs", "browser-vm-remote-vz-launcher.mjs"]) {
  test(`local readiness explicitly refuses ${name}`, t => {
    const f = fixture(t), launcher = path.join(f.root, "bin", name);
    fs.copyFileSync(f.launcher, launcher);
    fs.chmodSync(launcher, 0o755);
    assert.deepEqual(probe(f, { launcher }), unavailable("readiness_unsupported"));
    assert.equal(fs.existsSync(f.calls), false);
  });
}

for (const [label, response, reason] of [
  ["missing userspace backend", { schema: capabilitySchema, available: false, reason: "preparation_required" }, "preparation_required"],
  ["unqualified backend", { schema: capabilitySchema, available: false, reason: "host_unsupported" }, "host_unsupported"],
  ["unavailable KVM", { schema: capabilitySchema, available: false }, "host_unsupported"],
  ["wrong schema", { schema: "other", available: true }, "readiness_unsupported"],
  ["non-boolean availability", { schema: capabilitySchema, available: "true" }, "host_unsupported"],
  ["invalid JSON", "invalid", "readiness_unsupported"],
  ["wrong JSON type", [], "readiness_unsupported"],
]) {
  test(`native capability fails closed for ${label}`, t => {
    const f = fixture(t);
    assert.deepEqual(probe(f, { response }), unavailable(reason));
  });
}

test("failed or timed-out native capability probes stay unavailable", t => {
  const f = fixture(t);
  assert.deepEqual(probe(f, { status: 1 }), unavailable("readiness_unsupported"));
  assert.deepEqual(probe(f, { timeout: true }), unavailable("readiness_unsupported"));
});

for (const kind of ["missing", "nonexecutable"]) {
  test(`Linux native readiness requires TURN: ${kind}`, t => {
    const f = fixture(t);
    if (kind === "missing") fs.unlinkSync(f.turn);
    else fs.chmodSync(f.turn, 0o644);
    assert.deepEqual(probe(f), unavailable("preparation_required"));
    assert.equal(fs.existsSync(f.calls), false);
  });
}

for (const artifact of ["kernel", "image"]) {
  test(`host readiness checks ${artifact} shape while Runtime owns content verification`, t => {
    const f = fixture(t);
    fs.writeFileSync(f[artifact], Buffer.alloc(fs.statSync(f[artifact]).size, 0x78));
    assert.deepEqual(probe(f, { envOverrides: { FIXTURE_FORBID_HASH: "1" } }), ready);
  });
}

for (const [artifact, reason] of [["sidecar", "artifact_invalid"], ["initrd", "preparation_required"]]) {
  test(`host readiness checks ${artifact} presence before native readiness`, t => {
    const f = fixture(t);
    fs.unlinkSync(f[artifact]);
    assert.deepEqual(probe(f), unavailable(reason));
    assert.equal(fs.existsSync(f.calls), false);
  });
}

for (const artifact of ["image", "sidecar", "kernel", "initrd"]) {
  test(`host readiness checks ${artifact} read permission`, t => {
    const f = fixture(t);
    fs.chmodSync(f[artifact], 0o000);
    assert.deepEqual(probe(f), unavailable(["kernel", "initrd"].includes(artifact)
      ? "preparation_required" : "artifact_invalid"));
  });
}

test("host readiness leaves guest image inspection to the artifact gate", t => {
  const f = fixture(t), inspector = path.join(f.root, "debugfs"), calls = path.join(f.root, "guest-inspection");
  fs.writeFileSync(inspector, `#!/bin/sh\nprintf x >> "${calls}"\nexit 1\n`, { mode: 0o755 });
  assert.deepEqual(probe(f, { envOverrides: { ELASTOS_DEBUGFS_BIN: inspector, FIXTURE_FORBID_HASH: "1" } }), ready);
  assert.equal(fs.existsSync(calls), false);
});

test("host readiness leaves all verification claims false even with a forged cache file", t => {
  const f = fixture(t);
  fs.writeFileSync(path.join(f.root, "browser-vm/.elastos-verified-identity.json"),
    JSON.stringify({ verified_sidecar: true, ok: true }));
  const result = probe(f, { envOverrides: { FIXTURE_FORBID_HASH: "1" } });
  assert.deepEqual(result, ready);
  assert.equal(JSON.stringify(result).includes("verified"), false);
});

test("whole preflight validates image-set mode and configured image paths", t => {
  const f = fixture(t);
  assert.equal(probe(f, { args: ["--verify-image-set"] }).ok, true);
  const changed = path.join(f.root, "different-kernel");
  fs.writeFileSync(changed, "different kernel");
  const result = probe(f, { args: ["--verify-image-set"], expectedStatus: 1,
    envOverrides: { ELASTOS_BROWSER_VM_KERNEL: changed } });
  assert.equal(result.ok, false);
  assert.equal(result.reason, "artifact_invalid");
  assert.equal(fs.existsSync(f.calls), false);
});

test("whole shell rejects invalid modes and extra arguments", t => {
  const f = fixture(t);
  probe(f, { args: ["--unknown"], expectedStatus: 2 });
  probe(f, { args: ["--host-readiness", "extra"], expectedStatus: 2 });
  assert.equal(fs.existsSync(f.calls), false);
});

test("Linux keeps its crosvm artifact requirement", t => {
  const f = fixture(t);
  fs.unlinkSync(path.join(f.root, "bin/crosvm"));
  assert.deepEqual(probe(f), unavailable("preparation_required"));
  assert.equal(fs.existsSync(f.calls), false);
});

test("host identity and unsupported x86 local readiness fail before native probing", t => {
  const f = fixture(t);
  assert.deepEqual(probe(f, { hostSystem: "Darwin", hostMachine: "arm64" }), unavailable("host_unsupported"));
  assert.deepEqual(probe(f, { platform: "linux-amd64", hostMachine: "x86_64" }), unavailable("host_unsupported"));
  assert.equal(fs.existsSync(f.calls), false);
});
