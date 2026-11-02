import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("./test-behaviour.sh", import.meta.url));

function fixture(t, ci = "") {
  const root = mkdtempSync(join(tmpdir(), "behaviour-inputs-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const inputs = join(root, "inputs");
  const log = join(root, "commands.jsonl");
  const npmLog = join(root, "npm.json");
  const installer = `const fs = require("node:fs"), path = require("node:path");
    fs.appendFileSync(process.env.INPUTS_LOG, JSON.stringify({
      revision: __BUILD__, args: process.argv.slice(2),
      cache: process.env.PLAYWRIGHT_BROWSERS_PATH, gc: process.env.PLAYWRIGHT_SKIP_BROWSER_GC,
      installerNode: process.env.FIXTURE_NODE_VERSION || "host",
    }) + "\\n");
    if (process.env.FAIL_REVISION === String(__BUILD__)) process.exit(73);
    fs.mkdirSync(path.join(process.env.PLAYWRIGHT_BROWSERS_PATH, "shell-" + __BUILD__), { recursive: true });
  `;
  for (const [prefix, name, version, revision] of [
    ["browser", "playwright", "1.60.0", 1223],
    ["core", "playwright-core", "1.59.1", 1217],
  ]) {
    const packageRoot = join(inputs, prefix, "node_modules", name);
    mkdirSync(packageRoot, { recursive: true });
    writeFileSync(join(packageRoot, "package.json"), JSON.stringify({ version }));
    writeFileSync(join(packageRoot, "cli.js"), installer.replaceAll("__BUILD__", String(revision)));
    if (name === "playwright") {
      const bin = join(inputs, prefix, "node_modules", ".bin");
      mkdirSync(bin);
      writeFileSync(join(bin, "playwright"), `#!/usr/bin/env node\n${installer.replaceAll("__BUILD__", String(revision))}`, { mode: 0o755 });
    }
  }
  const nodeRoot = join(inputs, "core", "node_modules", "node");
  mkdirSync(join(nodeRoot, "bin"), { recursive: true });
  writeFileSync(join(nodeRoot, "package.json"), JSON.stringify({ version: "24.21.0" }));
  const nodeLauncher = `#!/bin/sh\nexport FIXTURE_NODE_VERSION=24.21.0\nexec "${process.execPath}" "$@"\n`;
  writeFileSync(join(nodeRoot, "bin", "node"), nodeLauncher, { mode: 0o755 });
  const bin = join(root, "bin");
  mkdirSync(bin);
  writeFileSync(join(bin, "npm"), `#!/usr/bin/env node
    const fs = require("node:fs"), path = require("node:path");
    const args = process.argv.slice(2);
    fs.writeFileSync(process.env.NPM_LOG, JSON.stringify(args));
    const nodeRoot = path.join(args[args.indexOf("--prefix") + 1], "node_modules", "node");
    fs.mkdirSync(path.join(nodeRoot, "bin"), { recursive: true });
    fs.writeFileSync(path.join(nodeRoot, "package.json"), JSON.stringify({ version: "24.21.0" }));
    fs.writeFileSync(path.join(nodeRoot, "bin", "node"), process.env.NODE_FIXTURE_BIN, { mode: 0o755 });
  `, { mode: 0o755 });
  const env = { ...process.env, CI: ci, BEHAVIOUR_INPUTS: inputs, INPUTS_LOG: log,
    NPM_LOG: npmLog, NODE_FIXTURE_BIN: nodeLauncher, PATH: `${bin}:${process.env.PATH}`,
    PLAYWRIGHT_BROWSERS_PATH: join(root, "cache"), PLAYWRIGHT_SKIP_BROWSER_GC: "0" };
  delete env.FAIL_REVISION;
  delete env.FIXTURE_NODE_VERSION;
  return { env, log, npmLog, nodeRoot, run: () => spawnSync("bash", [script, "inputs"], { env, encoding: "utf8", timeout: 10_000 }) };
}

for (const ci of ["", "true"]) {
  test(`behaviour inputs install both pinned browser builds${ci ? " in CI" : ""}`, t => {
    const { env, log, run } = fixture(t, ci);
    for (let attempt = 0; attempt < 2; attempt++) {
      const result = run();
      assert.equal(result.status, 0, result.stderr);
      const commands = readFileSync(log, "utf8").trim().split("\n").map(JSON.parse).slice(attempt * 2);
      assert.deepEqual(commands, [
        { revision: 1223, args: ["install", "chromium", "--only-shell", ...(ci ? ["--with-deps"] : [])], cache: env.PLAYWRIGHT_BROWSERS_PATH, gc: "1", installerNode: "host" },
        { revision: 1217, args: ["install", "chromium-headless-shell"], cache: env.PLAYWRIGHT_BROWSERS_PATH, gc: "1", installerNode: "24.21.0" },
      ]);
      assert.deepEqual(readdirSync(env.PLAYWRIGHT_BROWSERS_PATH).sort(), ["shell-1217", "shell-1223"]);
    }
  });
}

test("behaviour inputs fail when the older browser build cannot install", t => {
  const { env, run } = fixture(t);
  env.FAIL_REVISION = "1217";
  assert.equal(run().status, 73);
});

test("behaviour inputs install the pinned older CLI and installer runtime together", t => {
  const { env, npmLog, nodeRoot, run } = fixture(t);
  rmSync(nodeRoot, { recursive: true });
  const result = run();
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(readFileSync(npmLog, "utf8")), [
    "install", "--prefix", join(env.BEHAVIOUR_INPUTS, "core"), "--no-save", "--package-lock=false",
    "playwright-core@1.59.1", "node@24.21.0",
  ]);
});

test("behaviour inputs reject a missing pinned installer binary instead of using host Node", t => {
  const { nodeRoot, run } = fixture(t);
  rmSync(join(nodeRoot, "bin", "node"));
  const result = run();
  assert.notEqual(result.status, 0, result.stderr);
  assert.match(result.stderr, /node\/bin\/node.*(?:No such file|not found)/);
});
