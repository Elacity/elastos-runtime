/**
 * <Home> (W5b) — headless SSR snapshot tests for the FLEET landing surface.
 *
 * Renders the Home surface server-side from a `homeView`-built fleet and asserts the
 * moat invariant AT FLEET SCALE: a mixed fleet paints each capsule's two channels
 * independently and honestly, side by side, with NO blended "all systems green". A
 * verified-but-exhausted-and-tampered capsule sits next to an unsigned-but-clean one
 * and next to an unknown-trust one — each renders its own honest sub-states, and the
 * only fleet figure is the honest `needsAttention` count. The component is pure paint;
 * all logic lives in esp/home.ts (tested under node:test there).
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { compile } from "svelte/compiler";
import { render } from "svelte/server";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";

import { homeView } from "./build/home.js";

const srcUrl = new URL("./Home.svelte", import.meta.url);
const { js } = compile(readFileSync(srcUrl, "utf8"), { generate: "server", name: "Home" });
mkdirSync(new URL("./build/", import.meta.url), { recursive: true });
const outUrl = new URL("./build/Home.gen.mjs", import.meta.url);
writeFileSync(outUrl, js.code);
const { default: Home } = await import(outUrl.href);

const verifiedChain = { verified: true, records: 9, signer: "k", error: null };
const brokenChain = { verified: false, records: 0, signer: "k", error: "tamper" };

// A deliberately mixed fleet: the exact case the moat depends on.
const fleet = [
  // verified trust, BUT exhausted budget AND tampered chain — the dangerous-yet-blessed capsule.
  {
    capsule: { name: "vm-blessed", title: "Blessed", trust_state: "cid-with-manifest-signature" },
    spendBudget: { limit: 5, spent: 5, remaining: 0 },
    auditChain: brokenChain,
  },
  // unsigned trust, BUT clean custody — must never be dressed up by the green custody.
  {
    capsule: { name: "vm-scrappy", title: "Scrappy", trust_state: "local-dev" },
    spendBudget: { limit: 100, spent: 10, remaining: 90 },
    auditChain: verifiedChain,
  },
  // unknown trust + no budget + no chain — fail-honest across the board.
  {
    capsule: { name: "vm-mystery", title: "Mystery", trust_state: "some-future-state" },
    spendBudget: null,
    auditChain: null,
  },
];

function paint(f) {
  return render(Home, { props: { view: homeView(f) } }).body;
}

test("a mixed fleet paints every capsule's honest channels independently — no roll-up", () => {
  const body = paint(fleet);

  // All three capsules are present, in order.
  assert.ok(body.indexOf('data-name="vm-blessed"') < body.indexOf('data-name="vm-scrappy"'));
  assert.ok(body.indexOf('data-name="vm-scrappy"') < body.indexOf('data-name="vm-mystery"'));

  // vm-blessed: verified trust does NOT mask exhausted budget or tampered chain.
  assert.ok(body.includes('data-trust="verified"'), "blessed shows verified");
  assert.ok(body.includes("Budget exhausted"), "blessed's exhausted budget still paints");
  assert.ok(body.includes("Chain tampered"), "blessed's tampered chain still paints");

  // vm-scrappy: clean custody does NOT upgrade unsigned trust.
  assert.ok(body.includes('data-trust="unsigned"'), "scrappy stays unsigned");
  assert.ok(body.includes("Within budget"));
  assert.ok(body.includes("Chain verified"));

  // vm-mystery: unknown trust is fail-honest (unsigned); absence rendered as absence.
  assert.ok(body.includes("Unmetered"), "no budget ⇒ unmetered, not 0/0 all-clear");
  assert.ok(body.includes("No durable chain"), "no chain ⇒ absent, not verified");

  // The ONLY fleet figure is the honest attention count — two wrong capsules
  // (blessed: exhausted+broken; scrappy + mystery: unsigned). All three are flagged.
  assert.ok(body.includes('data-attention="3"'), "all three are in a wrong state");
  assert.ok(body.includes('data-total="3"'));
  assert.ok(body.includes("3 of 3 need attention"));

  // There is no blended green: the literal string an "all good" banner would use
  // must not appear anywhere in the fleet paint.
  assert.ok(!body.includes("All systems"), "no fleet-level all-clear affordance");
});

test("a fully-clean fleet shows 0 of N — derived from honest states, not an independent flag", () => {
  const clean = [
    {
      capsule: { name: "vm-good", title: "Good", trust_state: "cid-with-manifest-signature" },
      spendBudget: { limit: 100, spent: 1, remaining: 99 },
      auditChain: verifiedChain,
    },
  ];
  const body = paint(clean);
  assert.ok(body.includes('data-attention="0"'));
  assert.ok(body.includes("0 of 1 need attention"));
  // Still no "all good" banner — absence of attention is shown as a count, not reassurance.
  assert.ok(!body.includes("All systems"));
});

test("an empty fleet renders 0 of 0 with no rows", () => {
  const body = paint([]);
  assert.ok(body.includes('data-attention="0"'));
  assert.ok(body.includes('data-total="0"'));
  assert.ok(!body.includes('data-testid="capsule-row"'), "no rows for an empty fleet");
});
