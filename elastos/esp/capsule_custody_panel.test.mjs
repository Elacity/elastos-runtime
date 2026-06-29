/**
 * <CapsuleCustodyPanel> (W5b) — headless SSR snapshot tests.
 *
 * Compiles the Svelte component server-side and renders it with view-models built
 * by the headless `homeCustodyView` projection, asserting the HONEST states paint
 * honestly: an absent chain + unmetered budget render "No durable chain" /
 * "Unmetered" and NEVER a verified/green affordance; a tampered chain renders
 * "Chain tampered" and never "Chain verified". The component is pure paint — all
 * custody logic lives in esp/spend_audit.ts (tested under node:test there).
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { compile } from "svelte/compiler";
import { render } from "svelte/server";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";

import { homeCustodyView } from "./build/spend_audit.js";

// Compile the component once (server target) and load the generated module.
const srcUrl = new URL("./CapsuleCustodyPanel.svelte", import.meta.url);
const { js } = compile(readFileSync(srcUrl, "utf8"), {
  generate: "server",
  name: "CapsuleCustodyPanel",
});
mkdirSync(new URL("./build/", import.meta.url), { recursive: true });
const outUrl = new URL("./build/CapsuleCustodyPanel.gen.mjs", import.meta.url);
writeFileSync(outUrl, js.code);
const { default: CapsuleCustodyPanel } = await import(outUrl.href);

function paint(spendBudget, auditChain) {
  const view = homeCustodyView(spendBudget, auditChain);
  return render(CapsuleCustodyPanel, { props: { view } }).body;
}

const verified = { verified: true, records: 42, signer: "deadbeefkey", error: null };
const broken = { verified: false, records: 0, signer: "deadbeefkey", error: "tamper at seq 7" };

test("absent chain + unmetered budget render honest labels, never green/verified", () => {
  const body = paint(null, null);
  assert.ok(body.includes("Unmetered"), `expected Unmetered label: ${body}`);
  assert.ok(body.includes("No durable chain"), `expected absent-chain label: ${body}`);
  assert.ok(body.includes('data-state="unmetered"'), "spend channel must mark unmetered");
  assert.ok(body.includes('data-state="absent"'), "audit channel must mark absent");
  // The moat invariant: absence must NOT paint as verified.
  assert.ok(!body.includes("Chain verified"), "an absent chain must never render as verified");
  assert.ok(!body.includes('data-state="verified"'), "no verified state on an absent chain");
});

test("metered-ok + verified is the only all-green panel", () => {
  const body = paint({ limit: 100, spent: 30, remaining: 70 }, verified);
  assert.ok(body.includes("Within budget"));
  assert.ok(body.includes("Chain verified"));
  assert.ok(body.includes('data-state="ok"'));
  assert.ok(body.includes('data-state="verified"'));
  assert.ok(body.includes("30 / 100"), "the live spend detail is painted when metered");
});

test("exhausted spend + broken chain paints both alarms, never verified", () => {
  const body = paint({ limit: 100, spent: 100, remaining: 0 }, broken);
  assert.ok(body.includes("Budget exhausted"));
  assert.ok(body.includes("Chain tampered"));
  assert.ok(body.includes('data-state="exhausted"'));
  assert.ok(body.includes('data-state="broken"'));
  assert.ok(!body.includes("Chain verified"), "a tampered chain must never render as verified");
});
