/**
 * ESP v0 — capsule-detail composition conformance (W5b).
 *
 * Proves `capsuleDetailView` is a pure composition of the trust + custody channels,
 * and — the moat invariant — that the two channels stay INDEPENDENT: a verified
 * capsule still surfaces an exhausted budget / broken chain, and an unsigned capsule
 * is never dressed up by a clean custody panel.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { ChainAttestation } from "./ai_act_audit.js";
import { capsuleDetailView } from "./capsule_detail.js";
import { homeCustodyView, spendBudgetView, auditChainView } from "./spend_audit.js";
import { trustMaterial } from "./two_channel.js";

const verifiedChain: ChainAttestation = { verified: true, records: 9, signer: "k", error: null };
const brokenChain: ChainAttestation = { verified: false, records: 0, signer: "k", error: "tamper" };

function capsule(trust_state: string, over: { name?: string; title?: string } = {}) {
  return { name: over.name ?? "vm-player", title: over.title ?? "Player", trust_state };
}

describe("capsuleDetailView (Home capsule-detail composition)", () => {
  it("is a pure composition — fields are exactly the two channels", () => {
    const c = capsule("cid-with-manifest-signature");
    const v = capsuleDetailView(c, { limit: 100, spent: 30, remaining: 70 }, verifiedChain);
    assert.deepEqual(Object.keys(v).sort(), ["custody", "name", "title", "trust"]);
    assert.equal(v.name, "vm-player");
    assert.equal(v.title, "Player");
    assert.equal(v.trust, trustMaterial(c));
    assert.deepEqual(v.custody, homeCustodyView({ limit: 100, spent: 30, remaining: 70 }, verifiedChain));
    // No blended "overall" verdict was invented.
    assert.equal("overall" in v, false);
  });

  it("a VERIFIED capsule still surfaces exhausted budget + broken chain (channels independent)", () => {
    const v = capsuleDetailView(capsule("local-manifest-signature"), { limit: 5, spent: 5, remaining: 0 }, brokenChain);
    assert.equal(v.trust, "verified", "trust verdict is its own channel");
    assert.equal(v.custody.spend.state, "exhausted", "a verified capsule does not mask an exhausted budget");
    assert.equal(v.custody.audit.state, "broken", "a verified capsule does not mask a broken chain");
    assert.equal(v.custody.audit.verified, false);
  });

  it("an UNSIGNED capsule is not dressed up by a clean custody panel", () => {
    const v = capsuleDetailView(capsule("local-dev"), { limit: 100, spent: 10, remaining: 90 }, verifiedChain);
    assert.equal(v.trust, "unsigned", "custody cannot upgrade an unsigned trust verdict");
    assert.equal(v.custody.spend.state, "ok");
    assert.equal(v.custody.audit.state, "verified");
  });

  it("an unknown trust_state is fail-honest (unsigned), custody still projected", () => {
    const v = capsuleDetailView(capsule("some-future-state"), null, null);
    assert.equal(v.trust, "unsigned");
    assert.deepEqual(v.custody.spend, spendBudgetView(null));
    assert.deepEqual(v.custody.audit, auditChainView(null));
  });
});
