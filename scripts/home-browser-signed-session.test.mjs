import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";

const source = fs.readFileSync(new URL("./home-passkey-virtual-auth-smoke.mjs", import.meta.url), "utf8");
const start = source.indexOf("async function ensureSignedWithVirtualPasskey(");
const end = source.indexOf("\nasync function currentPasskey(", start);
function harness({ reuse = true, journey = true, refresh = { ok: true, homeToken: "scoped-home" } } = {}) {
  const calls = [];
  const context = vm.createContext({ assert, REUSE_SIGNED_HOME: reuse, CHECK_BROWSER_CONTROLLED_JOURNEY: journey,
    waitForHomeReady: async () => calls.push("ready"), homeState: async () => ({ authority: "signed" }),
    refreshCurrentHomeToken: async () => { calls.push("refresh"); return refresh; },
    waitForSignedHome: async () => calls.push("signed"), signOut: async () => calls.push("sign-out"),
    signBackIn: async () => { calls.push("sign-in"); return "new-home"; } });
  vm.runInContext(source.slice(start, end), context);
  return { calls, run: () => context.ensureSignedWithVirtualPasskey({}) };
}

test("controlled Browser journey refreshes signed Home authority without a logout", async () => {
  const h = harness(); const result = await h.run();
  assert.equal(result.mode, "reused-signed-home"); assert.equal(result.homeToken, "scoped-home");
  assert.deepEqual(h.calls, ["ready", "refresh", "signed"]);
});
test("default Home smoke retains its sign-out and passkey sign-in", async () => {
  const h = harness({ reuse: false }); const result = await h.run();
  assert.equal(result.mode, "existing-session"); assert.equal(result.homeToken, "new-home");
  assert.deepEqual(h.calls, ["ready", "sign-out", "sign-in"]);
});
for (const refresh of [{ ok: false, homeToken: "untrusted" }, { ok: true, homeToken: "" }]) {
  test("failed session refresh cannot supply Browser authority", async () => {
    const h = harness({ refresh }); await assert.rejects(h.run(), /session refresh failed/);
    assert.deepEqual(h.calls, ["ready", "refresh"]);
  });
}
test("session reuse is restricted to the controlled Browser journey", async () => {
  const h = harness({ journey: false }); await assert.rejects(h.run(), /requires a controlled/);
  assert.deepEqual(h.calls, ["ready"]);
});
