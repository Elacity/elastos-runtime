import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";

const source = await readFile(new URL("../capsules/home/browser/shell-core.js", import.meta.url), "utf8");
const session = { principal_id: "alice", session_id: "session-a", proof_binding_id: "proof-a" };

function fixture(fetch) {
  return vm.runInNewContext(`${source.replace(/^export /gm, "")}\n({ setHomeAuthorityToken, clearHomeAuthorityToken, createHomeBackgroundImageReader })`, {
    document: { querySelector: () => null }, fetch, AbortSignal,
  });
}

test("an existing reader uses renewed credentials after the original token expires", async () => {
  let now = 0;
  const hour = 3600;
  const expiry = new Map([["original", 12 * hour], ["renewed", 23 * hour]]);
  const sent = [];
  const home = fixture(async (_url, { headers }) => {
    const token = headers["x-elastos-home-token"];
    sent.push(token);
    return { ok: expiry.get(token) > now, blob: async () => "alice-image" };
  });
  home.setHomeAuthorityToken("original", session);
  const read = home.createHomeBackgroundImageReader();
  now = 11 * hour;
  home.setHomeAuthorityToken("renewed", session);
  now = 12 * hour + 1;
  assert.equal(await read(), "alice-image");
  assert.deepEqual(sent, ["renewed"]);
});

for (const [name, replacement] of [
  ["another person", { ...session, principal_id: "bob" }],
  ["another session", { ...session, session_id: "session-b" }],
  ["another proof", { ...session, proof_binding_id: "proof-b" }],
]) {
  test(`an old frame cannot read after Home changes to ${name}`, async () => {
    let requests = 0;
    const home = fixture(async () => { requests += 1; return { ok: true, blob: async () => "image" }; });
    home.setHomeAuthorityToken("original", session);
    const read = home.createHomeBackgroundImageReader();
    home.setHomeAuthorityToken("replacement", replacement);
    await assert.rejects(read(), /Home session changed/);
    assert.equal(requests, 0);
  });
}

test("sign-out retires readers even when the same identity signs in again", async () => {
  const home = fixture(async () => { throw new Error("retired reader sent a request"); });
  home.setHomeAuthorityToken("original", session);
  const read = home.createHomeBackgroundImageReader();
  home.clearHomeAuthorityToken();
  home.setHomeAuthorityToken("renewed", session);
  await assert.rejects(read(), /Home session changed/);
});

test("renewing the same session during a read keeps the result", async () => {
  let release;
  const home = fixture(async () => ({ ok: true, blob: () => new Promise((resolve) => { release = resolve; }) }));
  home.setHomeAuthorityToken("original", session);
  const pending = home.createHomeBackgroundImageReader()();
  await new Promise(setImmediate);
  home.setHomeAuthorityToken("renewed", session);
  release("alice-image");
  assert.equal(await pending, "alice-image");
});

test("a delayed result stays retired after an A-to-B-to-A session change", async () => {
  let release;
  const home = fixture(async () => ({ ok: true, blob: () => new Promise((resolve) => { release = resolve; }) }));
  home.setHomeAuthorityToken("original", session);
  const pending = home.createHomeBackgroundImageReader()();
  await new Promise(setImmediate);
  home.setHomeAuthorityToken("other", { ...session, principal_id: "bob" });
  home.setHomeAuthorityToken("renewed", session);
  release("alice-image");
  await assert.rejects(pending, /Home session changed/);
});
