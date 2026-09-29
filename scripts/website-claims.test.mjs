import test from "node:test";
import assert from "node:assert/strict";
import { validHostedReceipt } from "./website-claims.mjs";

const origin = "https://elastos.example";
const observedAt = "2026-09-10T12:00:00Z";
const now = Date.parse("2026-09-10T13:00:00Z");
const hostedReceipt = {
  status: "verified", version: "0.7.0-dev", source_commit: "b".repeat(40), source_tree: "c".repeat(40),
  binary_sha256: "a".repeat(64), components_sha256: "d".repeat(64),
  home_index_sha256: "e".repeat(64), site_index_sha256: "f".repeat(64),
  observed_at: observedAt, target: `${origin}/`, evidence: "/claims.json",
};

test("a hosted version requires its own complete dated deployment receipt", () => {
  for (const hosted of [null, { status: "unverified" }, { status: "verified", version: "0.7.1" }]) {
    assert.equal(validHostedReceipt(hosted, origin, now), false);
  }
  assert.equal(validHostedReceipt(hostedReceipt, origin, now), true);
  for (const key of Object.keys(hostedReceipt)) {
    const incomplete = { ...hostedReceipt };
    delete incomplete[key];
    assert.equal(validHostedReceipt(incomplete, origin, now), false, key);
  }
});

test("a hosted receipt belongs to the exact server and its own evidence file", () => {
  for (const value of ["https://another.example", "http://elastos.example", undefined]) {
    assert.equal(validHostedReceipt(hostedReceipt, value, now), false);
  }
  for (const evidence of ["https://another.example/claims.json", "//another.example/claims.json", "/claims.json?old", "/release.json"]) {
    assert.equal(validHostedReceipt({ ...hostedReceipt, evidence }, origin, now), false);
  }
});

test("future observations and malformed source/artifact identities remain unverified", () => {
  for (const change of [
    { observed_at: "2026-09-11T12:00:00Z" }, { observed_at: "invalid" },
    { observed_at: "2026-02-31T12:00:00Z" }, { observed_at: "2026-09-09T24:00:00Z" },
    { observed_at: "2026-09-10" }, { source_tree: "bad" }, { source_commit: "bad" },
    { binary_sha256: "bad" }, { components_sha256: "bad" }, { home_index_sha256: "bad" },
    { site_index_sha256: "bad" }, { version: "latest" }, { version: ["0.7.0-dev"] }, { status: "unverified" },
  ]) assert.equal(validHostedReceipt({ ...hostedReceipt, ...change }, origin, now), false);
});
