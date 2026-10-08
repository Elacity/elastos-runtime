import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import vm from "node:vm";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

const root = path.resolve(import.meta.dirname, "..");

test("release configuration binds every host to the image set and Runtime profile", () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "browser-release-config-"));
  try {
    for (const platform of ["darwin-arm64", "linux-arm64", "linux-amd64"]) {
      const data = path.join(temp, platform);
      execFileSync(process.execPath, [path.join(root, "scripts/browser-source-home-config.mjs"),
        "--data-dir", data, "--platform", platform, "--release-image"], { env: {
          ELASTOS_BROWSER_VM_PYTHON: path.join(data, "bin/python3"),
          ELASTOS_DEBUGFS_BIN: path.join(data, "bin/debugfs"),
        } });
      const adapter = JSON.parse(fs.readFileSync(path.join(data, "config/browser-engine-adapter.json")));
      const env = adapter.adapters[0].supervisor.env;
      for (const [key, name] of [["ROOTFS", "rootfs.ext4"], ["KERNEL", "vmlinux"],
        ["ROOTFS_MANIFEST", "browser-vm-rootfs-manifest.json"],
        [platform === "darwin-arm64" ? "INITRAMFS" : "INITRD", "initrd"]]) {
        assert.equal(env[`ELASTOS_BROWSER_VM_${key}`], path.join(data, "browser-vm/image-set", name));
      }
      assert.equal(adapter.adapters[0].network_mode, "runtime_net_only");
      assert.deepEqual(adapter.adapters[0].display_modes, ["webrtc_remote_display"]);
      assert.equal(env.ELASTOS_BROWSER_VM_PROFILE_ROOT, undefined);
      assert.equal(env.ELASTOS_BROWSER_VM_PYTHON, path.join(data, "bin/python3"));
      assert.equal(env.ELASTOS_DEBUGFS_BIN, path.join(data, "bin/debugfs"));
    }
  } finally {
    fs.rmSync(temp, { recursive: true, force: true });
  }
});

test("Linux launcher selects the image kernel while the capsule kernel retains ownership", () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "browser-release-kernel-"));
  try {
    fs.mkdirSync(path.join(temp, "bin"));
    fs.mkdirSync(path.join(temp, "browser-vm/image-set"), { recursive: true });
    fs.writeFileSync(path.join(temp, "browser-vm/image-set/rootfs.ext4"), "image");
    fs.symlinkSync(path.join(temp, "browser-vm/image-set/rootfs.ext4"), path.join(temp, "browser-vm/rootfs.ext4"));
    fs.writeFileSync(path.join(temp, "bin/vmlinux"), "capsule-owned kernel");
    const source = fs.readFileSync(path.join(root, "scripts/browser-vm-local-crosvm-launcher.mjs"), "utf8");
    // Exercise the launcher's artifact selection before device/network setup.
    const selection = source.slice(source.indexOf("  const rootfs = process.env.ELASTOS_BROWSER_VM_ROOTFS"),
      source.indexOf('  requireFile(crosvm, "crosvm")'));
    const selected = [];
    vm.runInNewContext(selection, { process: { env: {} }, fs, path, dataDir: temp,
      requireFile: (file, label) => selected.push([file, label]) });
    assert.deepEqual(selected.find(([, label]) => label === "Browser VM kernel"),
      [path.join(fs.realpathSync(temp), "browser-vm/image-set/vmlinux"), "Browser VM kernel"]);
    assert.equal(fs.readFileSync(path.join(temp, "bin/vmlinux"), "utf8"), "capsule-owned kernel");
  } finally {
    fs.rmSync(temp, { recursive: true, force: true });
  }
});
