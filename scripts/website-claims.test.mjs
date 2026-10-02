import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { validHostedReceipt, validIsolationClaims, HOSTED_RECEIPT_MAX_AGE_MS } from "./website-claims.mjs";

const origin = "https://elastos.example";
const observedAt = "2026-09-10T12:00:00Z";
const now = Date.parse("2026-09-10T13:00:00Z");
const hostedReceipt = {
  status: "verified", version: "0.7.0-dev", source_commit: "b".repeat(40), source_tree: "c".repeat(40),
  binary_sha256: "a".repeat(64), components_sha256: "d".repeat(64),
  home_index_sha256: "e".repeat(64), site_index_sha256: "f".repeat(64),
  observed_at: observedAt, target: `${origin}/`, evidence: "/claims.json",
};

const expected = { ...hostedReceipt };
const valid = (receipt, target = origin, time = now, artifacts = expected) => validHostedReceipt(receipt, target, time, artifacts);

test("a hosted version requires its own complete dated deployment receipt", () => {
  for (const hosted of [null, { status: "unverified" }, { status: "verified", version: "0.7.1" }]) {
    assert.equal(valid(hosted), false);
  }
  assert.equal(valid(hostedReceipt), true);
  for (const key of Object.keys(hostedReceipt)) {
    const incomplete = { ...hostedReceipt };
    delete incomplete[key];
    assert.equal(valid(incomplete), false, key);
  }
});

test("a hosted receipt belongs to the exact server and its own evidence file", () => {
  for (const value of ["https://another.example", "http://elastos.example", undefined]) {
    assert.equal(validHostedReceipt(hostedReceipt, value, now, expected), false);
  }
  for (const evidence of ["https://another.example/claims.json", "//another.example/claims.json", "/claims.json?old", "/release.json"]) {
    assert.equal(valid({ ...hostedReceipt, evidence }), false);
  }
});

test("future observations and malformed source/artifact identities remain unverified", () => {
  for (const change of [
    { observed_at: "2026-09-11T12:00:00Z" }, { observed_at: "invalid" },
    { observed_at: "2026-02-31T12:00:00Z" }, { observed_at: "2026-09-09T24:00:00Z" },
    { observed_at: "2026-09-10" }, { source_tree: "bad" }, { source_commit: "bad" },
    { binary_sha256: "bad" }, { components_sha256: "bad" }, { home_index_sha256: "bad" },
    { site_index_sha256: "bad" }, { version: "latest" }, { version: ["0.7.0-dev"] }, { status: "unverified" },
  ]) assert.equal(valid({ ...hostedReceipt, ...change }), false);
});


test("expired receipts and identities outside accepted installation stay unverified", () => {
  const observed = Date.parse(observedAt);
  assert.equal(valid(hostedReceipt, origin, observed + HOSTED_RECEIPT_MAX_AGE_MS), true);
  assert.equal(valid(hostedReceipt, origin, observed + HOSTED_RECEIPT_MAX_AGE_MS + 1), false);
  assert.equal(valid(hostedReceipt, origin, NaN), false);
  assert.equal(validHostedReceipt(hostedReceipt, origin, now), false);
  assert.equal(valid({ ...hostedReceipt, fully_isolated: true }), false);
  for (const key of ["version", "source_commit", "source_tree", "binary_sha256", "components_sha256", "home_index_sha256", "site_index_sha256"]) {
    const mismatch = { ...expected, [key]: key === "version" ? "0.7.1" : "1".repeat(key.startsWith("source_") ? 40 : 64) };
    assert.equal(valid(hostedReceipt, origin, now, mismatch), false, key);
    const incomplete = { ...expected };
    delete incomplete[key];
    assert.equal(valid(hostedReceipt, origin, now, incomplete), false, key);
  }
});

test("isolation claims refuse stronger guarantees than the source boundary", () => {
  const facts = JSON.parse(readFileSync(new URL("../website/elastos/claims.json", import.meta.url)));
  assert.equal(validIsolationClaims(facts.isolation), true);
  for (const [key, value] of Object.entries({ apps: "all-wasm", home: "delegation-only", providers: "fully-confined", stolen_device_key: "same-identity-recovery", stored_keys: "hardware-protected", seed_operator: "cannot-read", proof: "live-accepted" })) {
    assert.equal(validIsolationClaims({ ...facts.isolation, [key]: value }), false, key);
  }
  assert.equal(validIsolationClaims({ ...facts.isolation, fully_isolated: true }), false);
});
