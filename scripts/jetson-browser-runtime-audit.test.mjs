import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { remoteAuditScript, verifyNativeComponents } from "./jetson-browser-runtime-audit.mjs";

const source = fileURLToPath(new URL("..", import.meta.url));
function nativeFixture(t) {
  const data = fs.mkdtempSync(path.join(os.tmpdir(), "browser-audit-native-"));
  t.after(() => fs.rmSync(data, { recursive: true, force: true }));
  fs.mkdirSync(path.join(data, "bin"));
  const manifest = { external: {} };
  for (const name of ["browser-vz-engine-supervisor", "crosvm", "vhost-device-vsock"]) {
    const content = `fixture ${name}\n`;
    fs.writeFileSync(path.join(data, "bin", name), content);
    manifest.external[name] = { install_path: `bin/${name}`, platforms: { "linux-arm64": {
      checksum: "sha256:" + crypto.createHash("sha256").update(content).digest("hex"),
    } } };
  }
  const save = () => fs.writeFileSync(path.join(data, "components.json"), JSON.stringify(manifest));
  save();
  return { data, manifest, save };
}

test("userspace admission checks all three installed binaries against their manifest", t => {
  const f = nativeFixture(t);
  const proof = verifyNativeComponents(f.data, source, "userspace-vsock");
  assert.equal(proof.ok, true, JSON.stringify(proof));
  assert.deepEqual(proof.components, ["browser-vz-engine-supervisor", "crosvm", "vhost-device-vsock"]);
  fs.appendFileSync(path.join(f.data, "bin/vhost-device-vsock"), "changed");
  assert.equal(verifyNativeComponents(f.data, source, "userspace-vsock").ok, false);
});

test("kernel transport has no userspace backend dependency", t => {
  const f = nativeFixture(t);
  fs.unlinkSync(path.join(f.data, "bin/vhost-device-vsock"));
  assert.equal(verifyNativeComponents(f.data, source, "kernel-vsock").ok, true);
  assert.equal(verifyNativeComponents(f.data, source, "userspace-vsock").ok, false);
  assert.equal(verifyNativeComponents(f.data, source, null).ok, false);
});

test("a receipt for another path cannot admit the selected native owner", t => {
  const f = nativeFixture(t);
  const component = f.manifest.external["browser-vz-engine-supervisor"];
  component.install_path = "bin/other";
  fs.copyFileSync(path.join(f.data, "bin/browser-vz-engine-supervisor"), path.join(f.data, "bin/other"));
  f.save();
  const proof = verifyNativeComponents(f.data, source, "kernel-vsock");
  assert.equal(proof.ok, false);
  assert.match(proof.error, /selected path/);
});

test("shipped archive packages are admitted through their receipt and bin link", t => {
  const f = nativeFixture(t);
  for (const name of ["crosvm", "vhost-device-vsock"]) {
    const install = `libexec/browser/${name}/pin/linux-arm64`;
    const archive = "sha256:" + crypto.createHash("sha256").update(`archive ${name}`).digest("hex");
    fs.mkdirSync(path.join(f.data, install, "bin"), { recursive: true });
    fs.renameSync(path.join(f.data, "bin", name), path.join(f.data, install, "bin", name));
    fs.chmodSync(path.join(f.data, install, "bin", name), 0o755);
    fs.symlinkSync(path.join(f.data, install, "bin", name), path.join(f.data, "bin", name));
    fs.writeFileSync(path.join(f.data, install, ".elastos-artifact-sha256"), archive.slice(7) + "\n");
    f.manifest.external[name] = { platforms: { "linux-arm64": {
      install_path: install, extract_path: `${name}-pin`, binary_path: `bin/${name}`, checksum: archive,
    } } };
  }
  f.save();
  const proof = verifyNativeComponents(f.data, source, "userspace-vsock");
  assert.equal(proof.ok, true, JSON.stringify(proof));
  // A receipt for other bytes, or a bin link elsewhere, is refused.
  const receipt = path.join(f.data, "libexec/browser/crosvm/pin/linux-arm64/.elastos-artifact-sha256");
  const saved = fs.readFileSync(receipt, "utf8");
  fs.writeFileSync(receipt, "b".repeat(64));
  assert.equal(verifyNativeComponents(f.data, source, "userspace-vsock").ok, false);
  fs.writeFileSync(receipt, saved);
  fs.unlinkSync(path.join(f.data, "bin/crosvm"));
  fs.symlinkSync(path.join(f.data, "bin/browser-vz-engine-supervisor"), path.join(f.data, "bin/crosvm"));
  assert.match(verifyNativeComponents(f.data, source, "userspace-vsock").error, /selected path/);
});

test("the real target audit reports native admission and drops retired helper requirements", t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "browser-audit-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.mkdirSync(path.join(root, "config"));
  fs.writeFileSync(path.join(root, "config/browser-engine-adapter.json"), JSON.stringify({ adapters: [{
    supervisor: { env: { ELASTOS_BROWSER_VM_CONTROL_LAUNCHER: path.join(root, "bin/browser-vz-engine-supervisor") } },
  }] }));
  const script = remoteAuditScript(root, fileURLToPath(new URL("..", import.meta.url)), 0);
  const result = spawnSync(process.execPath, ["-"], { input: script, encoding: "utf8", timeout: 10000 });
  assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(result.stdout);
  assert.equal(report.checks.native_owner_selected, true);
  assert.equal(report.checks.native_program_paths, true);
  assert.equal(report.checks.native_components_verified, false, "missing native receipt fails closed");
  assert.equal(report.checks.rootfs_pool_required, undefined);
  assert.equal(report.helper_hashes["browser-vm-local-crosvm-launcher.mjs"], undefined);
  assert.equal(report.helper_hashes["browser-vm-prepare-rootfs-pool.mjs"], undefined);
  assert.equal(report.preflight.readiness.state, "unavailable");

  const configPath = path.join(root, "config/browser-engine-adapter.json");
  const config = JSON.parse(fs.readFileSync(configPath, "utf8"));
  config.adapters[0].supervisor.env.ELASTOS_BROWSER_VM_VHOST_DEVICE_VSOCK_BIN = "/other/backend";
  fs.writeFileSync(configPath, JSON.stringify(config));
  const other = spawnSync(process.execPath, ["-"], { input: script, encoding: "utf8", timeout: 10000 });
  assert.equal(other.status, 0, other.stderr);
  assert.equal(JSON.parse(other.stdout).checks.native_program_paths, false);
});
