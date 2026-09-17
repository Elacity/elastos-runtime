import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

const source = fs.readFileSync(
  new URL("./browser-vm-control-service.mjs", import.meta.url),
  "utf8",
);
const match = source.match(
  /function persistentLauncherReadyLine\(stdout\) \{[\s\S]*?\n\}/,
);
assert.ok(match, "persistentLauncherReadyLine must exist");
const persistentLauncherReadyLine = new Function(
  `${match[0]}; return persistentLauncherReadyLine;`,
)();

test("persistent launcher ignores debugfs stdout before supervisor JSON", () => {
  const result = {
    schema: "elastos.browser.engine.supervisor-result/v1",
    page_id: "page:ready",
  };
  assert.equal(
    persistentLauncherReadyLine(`Allocated inode: 728\n${JSON.stringify(result)}\n`),
    JSON.stringify(result),
  );
  assert.equal(persistentLauncherReadyLine("Allocated inode: 728\n"), null);
});
