import test from "node:test";
import assert from "node:assert/strict";
import { assessRelease, assessInstall, hostedStatus, loadRelease } from "../website/elastos/site.mjs";

const cid = "QmVLFNQfW6V2LuXCX5xAq1jUmQrReE294Fb2NvETWgNbRk";
const publisher = "did:key:test-fixture";
const artifact = { cid, sha256: "a".repeat(64), size: 1024 };
const facts = {
  candidate: { version: "0.7.1" },
  public_install_proofs: [{ version: "0.7.1", platform: "x86_64-linux", source_commit: "a".repeat(40), evidence: "https://github.com/Elacity/elastos-runtime/blob/example/proof.md" }],
};
function documents(version = "0.7.1") {
  return [
    { payload: { schema: "elastos.release.head/v1", version, channel: "stable", signer_did: publisher, updated_at: 1776304142, latest_release_cid: cid }, signature: "a".repeat(128), signer_did: publisher },
    { payload: { schema: "elastos.release/v1", version, channel: "stable", released_at: 1776304142, platforms: { "x86_64-linux": { binary: structuredClone(artifact), components: structuredClone(artifact) } } }, signature: "b".repeat(128), signer_did: publisher },
  ];
}

test("matching metadata and a version-only proof cannot enable installation", () => {
  const state = assessInstall(assessRelease(...documents()), "x86_64-linux", facts);
  assert.equal(state.enabled, false);
  assert.equal(state.command, undefined);
});

test("a same-version artifact replacement cannot reuse old install proof", () => {
  const pair = documents();
  pair[1].payload.platforms["x86_64-linux"].binary.sha256 = "f".repeat(64);
  assert.equal(assessRelease(...pair).state, "matched");
  assert.equal(assessInstall(assessRelease(...pair), "x86_64-linux", facts).enabled, false);
});

test("legacy release remains distinct from the current Home candidate", () => {
  const metadata = assessRelease(...documents("0.1.2"));
  assert.equal(metadata.state, "matched");
  assert.equal(metadata.version, "0.1.2");
  assert.equal(assessInstall(metadata, "x86_64-linux", facts).enabled, false);
});

test("a Linux-only release cannot offer an Apple silicon installer", () => {
  assert.equal(assessInstall(assessRelease(...documents()), "aarch64-darwin", facts).enabled, false);
});

test("metadata alone cannot claim installed Home success", () => {
  const metadata = assessRelease(...documents());
  for (const evidence of [null, {}, { candidate: { version: "0.7.1" }, public_install_proofs: [] }, { ...facts, public_install_proofs: [{ ...facts.public_install_proofs[0], version: "0.7.0" }] }]) {
    assert.equal(assessInstall(metadata, "x86_64-linux", evidence).enabled, false);
  }
});

for (const [name, mutate] of [
  ["version mismatch", ([, release]) => { release.payload.version = "0.7.0"; }],
  ["channel mismatch", ([, release]) => { release.payload.channel = "preview"; }],
  ["publisher mismatch", ([, release]) => { release.signer_did = "did:key:another-fixture"; }],
  ["head publisher mismatch", ([head]) => { head.payload.signer_did = "did:key:another-fixture"; }],
  ["missing envelope signature", ([head]) => { delete head.signature; }],
  ["malformed release signature", ([, release]) => { release.signature = "invalid"; }],
  ["missing release CID", ([head]) => { delete head.payload.latest_release_cid; }],
  ["missing checksum", ([, release]) => { delete release.payload.platforms["x86_64-linux"].binary.sha256; }],
  ["missing component manifest", ([, release]) => { delete release.payload.platforms["x86_64-linux"].components; }],
  ["invalid platform size", ([, release]) => { release.payload.platforms["x86_64-linux"].binary.size = -1; }],
  ["impossible timestamp", ([, release]) => { release.payload.released_at = 1e30; }],
  ["release newer than head", ([, release]) => { release.payload.released_at += 1; }],
]) {
  test(`${name} leaves the install action disabled`, () => {
    const pair = documents();
    mutate(pair);
    const metadata = assessRelease(...pair);
    assert.equal(metadata.state, "inconsistent");
    assert.equal(assessInstall(metadata, "x86_64-linux", facts).enabled, false);
  });
}

test("release loading requests only the two same-origin documents without session cookies", async () => {
  const calls = [];
  const pair = documents();
  const metadata = await loadRelease(async (path, options) => {
    calls.push(path);
    assert.equal(options.credentials, "omit");
    assert.equal(options.cache, "no-store");
    assert.ok(options.signal);
    return { ok: true, json: async () => pair[path === "/release-head.json" ? 0 : 1] };
  });
  assert.deepEqual(calls.sort(), ["/release-head.json", "/release.json"]);
  assert.equal(metadata.state, "matched");
});

test("missing, failed and unreadable release responses leave installation unavailable", async () => {
  for (const fetcher of [
    async () => { throw new Error("network unavailable"); },
    async () => ({ ok: false, status: 404 }),
    async () => ({ ok: true, json: async () => { throw new SyntaxError("invalid JSON"); } }),
    async () => ({ ok: true, json: async () => null }),
  ]) {
    const metadata = await loadRelease(fetcher);
    assert.equal(metadata.state, "unavailable");
    assert.equal(assessInstall(metadata, "x86_64-linux", facts).enabled, false);
  }
});

test("a hosted version requires its own commit and deployment evidence", () => {
  for (const hosted of [null, { status: "unverified" }, { status: "verified", version: "0.7.1" }]) {
    assert.equal(hostedStatus(hosted).text, "Version awaiting verification");
  }
  const hosted = { status: "verified", version: "0.7.1-dev", source_commit: "b".repeat(40), evidence: facts.public_install_proofs[0].evidence };
  assert.deepEqual(hostedStatus(hosted), { text: "0.7.1-dev · bbbbbbbb", evidence: hosted.evidence });
});
