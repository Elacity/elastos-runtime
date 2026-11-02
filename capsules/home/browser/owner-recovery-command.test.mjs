import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import test from "node:test";

import { ownerRecoveryCommand } from "./owner-recovery-command.js";

test("recovery command names this page's origin and rp id", () => {
  assert.equal(
    ownerRecoveryCommand(new URL("http://localhost:8090/home/")),
    "elastos identity arm-owner-recovery --origin 'http://localhost:8090' --rp-id 'localhost'",
  );
  assert.equal(
    ownerRecoveryCommand(new URL("https://home.example:8443/")),
    "elastos identity arm-owner-recovery --origin 'https://home.example:8443' --rp-id 'home.example'",
  );
});

test("a hostile origin or host reaches the command only as literal arguments", () => {
  const hostile = { origin: "http://x$(touch pwned)`id`'; echo y", hostname: "a'b$(id)" };
  const command = ownerRecoveryCommand(hostile).replace("elastos", "printf '%s\\n'");
  const args = execFileSync("/bin/sh", ["-c", command], { encoding: "utf8" }).split("\n").slice(0, -1);
  assert.deepEqual(args, ["identity", "arm-owner-recovery", "--origin", hostile.origin, "--rp-id", hostile.hostname]);
});
