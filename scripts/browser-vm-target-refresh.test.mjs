import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";

const source = fileURLToPath(new URL("..", import.meta.url));
function fixture(t, { system = "Darwin", arch = "arm64" } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "browser-refresh-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const data = path.join(root, "data");
  const commands = path.join(root, "commands");
  fs.mkdirSync(path.join(data, "bin"), { recursive: true });
  fs.mkdirSync(commands);
  // Refresh does not launch a VM. Exercise the supported Mac branch on either test host.
  fs.writeFileSync(path.join(commands, "uname"), `#!/bin/sh\ncase "$1" in -s) echo ${system};; -m) echo ${arch};; *) exit 1;; esac\n`, { mode: 0o755 });
  const native = path.join(data, "bin/browser-vz-engine-supervisor");
  fs.writeFileSync(native, "fixture native binary\n", { mode: 0o755 });
  const checksum = "sha256:" + crypto.createHash("sha256").update(fs.readFileSync(native)).digest("hex");
  const manifest = { external: { "browser-vz-engine-supervisor": {
    install_path: "bin/browser-vz-engine-supervisor",
    platforms: { "darwin-arm64": { checksum } },
  } } };
  fs.writeFileSync(path.join(data, "components.json"), JSON.stringify(manifest));
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("ELASTOS_")));
  const refresh = (...args) => spawnSync("bash", [path.join(source, "scripts/browser-vm-target-refresh.sh"),
    "--source-dir", source, "--data-dir", data, "--node-bin", process.execPath, ...args], {
    encoding: "utf8", timeout: 10000, env: { ...env, PATH: `${commands}:${process.env.PATH}` },
  });
  return { data, native, refresh };
}

test("refresh preserves admitted native bytes and installs no retired launcher or pool", t => {
  const f = fixture(t);
  const binary = fs.readFileSync(f.native);
  const manifest = fs.readFileSync(path.join(f.data, "components.json"));
  const result = f.refresh();
  assert.equal(result.status, 0, result.stdout + result.stderr);
  for (const file of ["bin/browser-vm-local-crosvm-launcher", "bin/browser-vm-local-crosvm-launcher.mjs",
    "bin/browser-vm-prepare-rootfs-pool", "scripts/browser-vm-prepare-rootfs-pool.mjs"]) {
    assert.equal(fs.existsSync(path.join(f.data, file)), false, file);
  }
  assert.deepEqual(fs.readFileSync(f.native), binary);
  assert.deepEqual(fs.readFileSync(path.join(f.data, "components.json")), manifest);
  const verify = f.refresh("--verify-only");
  assert.equal(verify.status, 0, verify.stdout + verify.stderr);
});

test("refresh refuses native checksum drift before changing helpers", t => {
  const f = fixture(t);
  fs.appendFileSync(f.native, "changed\n");
  const result = f.refresh();
  assert.equal(result.status, 1, result.stdout + result.stderr);
  assert.match(result.stderr, /checksum mismatch/);
  assert.equal(fs.existsSync(path.join(f.data, "scripts")), false);
  assert.equal(fs.existsSync(path.join(f.data, "backups")), false);
});

test("an unsupported host requires an explicit remote Engine before refresh", t => {
  const f = fixture(t, { system: "Linux", arch: "x86_64" });
  const result = f.refresh();
  assert.equal(result.status, 1, result.stdout + result.stderr);
  assert.match(result.stderr, /approved remote Browser Engine/);
  assert.equal(fs.existsSync(path.join(f.data, "scripts")), false);
});

test("an explicitly configured remote Engine preserves gateway authority", t => {
  const f = fixture(t, { system: "Linux", arch: "x86_64" });
  fs.unlinkSync(f.native);
  fs.unlinkSync(path.join(f.data, "components.json"));
  fs.mkdirSync(path.join(f.data, "config"));
  const configPath = path.join(f.data, "config/browser-engine-adapter.json");
  const config = JSON.stringify({ adapters: [{ supervisor: { env: {
    ELASTOS_BROWSER_VM_CONTROL_LAUNCHER: path.join(f.data, "bin/browser-vm-remote-vz-launcher"),
  } } }] });
  fs.writeFileSync(configPath, config);
  const result = f.refresh();
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(fs.readFileSync(configPath, "utf8"), config);
  assert.equal(fs.existsSync(path.join(f.data, "bin/browser-vm-remote-vz-launcher")), true);
  assert.equal(fs.existsSync(f.native), false);
});
