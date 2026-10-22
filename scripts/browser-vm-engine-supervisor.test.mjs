import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { localControlLauncherForPlatform } from "./browser-vm-engine-supervisor.mjs";

function launcher(t, platform, env = {}) {
  for (const key of ["ELASTOS_BROWSER_VM_CONTROL_LAUNCHER", "ELASTOS_BROWSER_VM_VZ_SUPERVISOR"]) {
    const previous = process.env[key];
    t.after(() => previous === undefined ? delete process.env[key] : process.env[key] = previous);
    if (env[key] === undefined) delete process.env[key];
    else process.env[key] = env[key];
  }
  return localControlLauncherForPlatform({ dataDir: "/fixture/home", platform });
}

for (const platform of ["linux-arm64", "darwin-arm64"]) {
  test(`${platform} defaults to the shared native owner`, t => {
    assert.equal(launcher(t, platform), "/fixture/home/bin/browser-vz-engine-supervisor");
  });
}

for (const platform of ["linux-amd64", "darwin-amd64", "windows-arm64"]) {
  test(`${platform} has no implicit local owner`, t => {
    assert.equal(launcher(t, platform), "");
  });
}

test("an explicitly selected remote Engine keeps authority on a gateway", t => {
  assert.equal(launcher(t, "linux-amd64", {
    ELASTOS_BROWSER_VM_CONTROL_LAUNCHER: "/fixture/home/bin/browser-vm-remote-vz-launcher",
  }), "/fixture/home/bin/browser-vm-remote-vz-launcher");
});

test("Mac preserves its explicit native supervisor path", t => {
  assert.equal(launcher(t, "darwin-arm64", {
    ELASTOS_BROWSER_VM_VZ_SUPERVISOR: "/fixture/selected/browser-vz-engine-supervisor",
  }), "/fixture/selected/browser-vz-engine-supervisor");
});

test("the actual prewarm command explains an unsupported local host", t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "browser-owner-default-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("ELASTOS_")));
  const result = spawnSync(process.execPath, [fileURLToPath(new URL("./browser-vm-engine-supervisor.mjs", import.meta.url))], {
    encoding: "utf8", timeout: 3000, env: { ...env,
      ELASTOS_BROWSER_VM_PLATFORM: "linux-amd64", ELASTOS_BROWSER_VM_DATA_DIR: root,
      ELASTOS_BROWSER_VM_CONTROL_SOCKET: path.join(root, "control.sock"),
      ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE: "1",
    },
  });
  assert.equal(result.status, 1, result.stderr);
  const error = JSON.parse(result.stderr.trim());
  assert.equal(error.code, "host_unsupported");
  assert.match(error.message, /approved remote Browser Engine/);
  assert.equal(fs.existsSync(path.join(root, "control.sock.start.lock")), false);
});
