import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import test from "node:test";

const repo = new URL("../", import.meta.url);
const smoke = "node scripts/library-shared-cid-smoke.mjs";

test("shared CID smoke runs in the browser recipe beside the Library layout smoke", () => {
  // Just prints dry-run commands to stderr.
  const commands = (recipe) => {
    const { stderr, status } = spawnSync("just", ["--dry-run", recipe], { cwd: repo, encoding: "utf8" });
    assert.equal(status, 0, stderr);
    return stderr;
  };
  assert.ok(!commands("product-ui-source").includes(smoke));
  assert.ok(commands("product-ui-browser").includes(`node scripts/library-product-layout-smoke.mjs\n${smoke}\n`));
});

test("shared CID smoke uses the same Playwright and Brave setup as the Library layout smoke", () => {
  const source = readFileSync(new URL("scripts/library-shared-cid-smoke.mjs", repo), "utf8");
  const layout = readFileSync(new URL("scripts/library-product-layout-smoke.mjs", repo), "utf8");
  for (const line of layout.split("\n").filter(line => /^(import \{ createRequire|const (brave|require|\{ chromium \}))/.test(line))) {
    assert.ok(source.includes(line), `shared browser setup: ${line}`);
  }
  assert.match(source, /chromium\.launch\(\{ headless: true, executablePath: brave \}\)/);
  assert.doesNotMatch(source, /ELASTOS_PLAYWRIGHT_MODULE|ELASTOS_BROWSER_EXECUTABLE/);
});
