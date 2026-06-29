/**
 * <CapsuleDetail> (W5b) — headless SSR snapshot tests.
 *
 * Renders the capsule-detail surface server-side from `capsuleDetailView`-built
 * view-models and asserts the two channels paint INDEPENDENTLY and honestly: a
 * verified capsule with an exhausted budget + broken chain shows "Verified" yet ALSO
 * "Budget exhausted" / "Chain tampered" (never masked); an unsigned capsule shows
 * "Unsigned" even with a clean custody panel. The component is pure paint — all logic
 * lives in esp/capsule_detail.ts (tested under node:test there).
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { compile } from "svelte/compiler";
import { render } from "svelte/server";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";

import { capsuleDetailView } from "./build/capsule_detail.js";

const srcUrl = new URL("./CapsuleDetail.svelte", import.meta.url);
const { js } = compile(readFileSync(srcUrl, "utf8"), { generate: "server", name: "CapsuleDetail" });
mkdirSync(new URL("./build/", import.meta.url), { recursive: true });
const outUrl = new URL("./build/CapsuleDetail.gen.mjs", import.meta.url);
writeFileSync(outUrl, js.code);
const { default: CapsuleDetail } = await import(outUrl.href);

const verifiedChain = { verified: true, records: 9, signer: "k", error: null };
const brokenChain = { verified: false, records: 0, signer: "k", error: "tamper" };

function paint(trust_state, spend, audit) {
  const view = capsuleDetailView({ name: "vm-player", title: "Player", trust_state }, spend, audit);
  return render(CapsuleDetail, { props: { view } }).body;
}

test("a verified capsule still paints exhausted budget + broken chain (independent channels)", () => {
  const body = paint("cid-with-manifest-signature", { limit: 5, spent: 5, remaining: 0 }, brokenChain);
  assert.ok(body.includes('data-trust="verified"'), "trust channel marks verified");
  assert.ok(body.includes("Verified"));
  // Custody is NOT masked by the verified trust badge.
  assert.ok(body.includes("Budget exhausted"), "exhausted budget must still paint");
  assert.ok(body.includes('data-state="exhausted"'));
  assert.ok(body.includes("Chain tampered"), "broken chain must still paint");
  assert.ok(body.includes('data-state="broken"'));
  assert.ok(!body.includes("Chain verified"), "a tampered chain must never render verified");
});

test("an unsigned capsule paints Unsigned even with a clean custody panel", () => {
  const body = paint("local-dev", { limit: 100, spent: 10, remaining: 90 }, verifiedChain);
  assert.ok(body.includes('data-trust="unsigned"'), "custody cannot upgrade the trust badge");
  assert.ok(body.includes("Unsigned"));
  assert.ok(body.includes("Within budget"));
  assert.ok(body.includes("Chain verified"));
});

test("an unknown trust_state is painted as Unsigned (fail-honest), custody honest too", () => {
  const body = paint("some-future-state", null, null);
  assert.ok(body.includes('data-trust="unsigned"'), "unknown trust never over-trusted");
  assert.ok(body.includes("Unmetered"));
  assert.ok(body.includes("No durable chain"));
  assert.ok(!body.includes("Chain verified"));
});
