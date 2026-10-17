import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("./test-behaviour.sh", import.meta.url));
const controlSmoke = "scripts/browser-selkies-control-service-smoke.sh";

for (const smokeExit of [0, 75]) {
  test(`behaviour runner ${smokeExit ? "stops after a failed" : "runs the"} control-only process smoke`, t => {
    const root = mkdtempSync(join(tmpdir(), "behaviour-control-"));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const inputs = join(root, "inputs"), bin = join(root, "bin"), log = join(root, "commands.jsonl");
    mkdirSync(bin);
    for (const [prefix, name, version] of [
      ["browser", "playwright", "1.60.0"], ["core", "playwright-core", "1.59.1"],
    ]) {
      const packageRoot = join(inputs, prefix, "node_modules", name);
      mkdirSync(packageRoot, { recursive: true });
      writeFileSync(join(packageRoot, "package.json"), JSON.stringify({ version }));
    }
    const shim = `#!${process.execPath}
      const fs = require("node:fs"), childProcess = require("node:child_process");
      const args = process.argv.slice(2);
      if (args[0] === "-p") {
        const result = childProcess.spawnSync(process.execPath, args, { stdio: "inherit" });
        process.exit(result.status ?? 1);
      }
      fs.appendFileSync(process.env.BEHAVIOUR_COMMAND_LOG, JSON.stringify(args) + "\\n");
      process.exit(args[0] === ${JSON.stringify(controlSmoke)} ? Number(process.env.CONTROL_SMOKE_EXIT) : 0);
    `;
    // Only test processes are replaced; the actual shell runner and installed
    // package checks decide which commands execute and which failures stop it.
    writeFileSync(join(bin, "node"), shim, { mode: 0o755 });
    writeFileSync(join(bin, "bash"), shim, { mode: 0o755 });
    const result = spawnSync("/bin/bash", [script], {
      env: { ...process.env, BEHAVIOUR_INPUTS: inputs, BEHAVIOUR_COMMAND_LOG: log,
        CONTROL_SMOKE_EXIT: String(smokeExit), PATH: `${bin}:${process.env.PATH}` },
      encoding: "utf8", timeout: 10_000,
    });
    assert.equal(result.status, smokeExit, result.stderr);
    const commands = readFileSync(log, "utf8").trim().split("\n").map(JSON.parse);
    assert.deepEqual(commands.filter(args => args[0] === controlSmoke), [[controlSmoke, "--control-only"]]);
    assert.equal(commands.at(-1)[0], smokeExit ? controlSmoke : "--test",
      "a failed control smoke stops every later check; success reaches the Node suite");
  });
}
