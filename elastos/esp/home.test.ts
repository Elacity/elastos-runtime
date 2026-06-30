/**
 * ESP v0 — Home fleet composition conformance (W5b).
 *
 * Proves `homeView` is a pure composition of `capsuleDetailView` over the fleet (each
 * capsule carried through verbatim, in input order) and that the ONLY fleet-level
 * figure — `needsAttention` — is monotonic toward caution: it counts exactly the
 * unambiguously-wrong sub-states and can never manufacture an "all clear". No
 * cross-capsule roll-up verdict is invented.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { ChainAttestation } from "./ai_act_audit.js";
import { capsuleDetailView } from "./capsule_detail.js";
import {
  homeView,
  capsuleNeedsAttention,
  homeCapsules,
  isHomeCapsule,
  type CapsuleFleetEntry,
} from "./home.js";

const verifiedChain: ChainAttestation = { verified: true, records: 9, signer: "k", error: null };
const brokenChain: ChainAttestation = { verified: false, records: 0, signer: "k", error: "tamper" };

function entry(
  trust_state: string,
  spendBudget: CapsuleFleetEntry["spendBudget"],
  auditChain: CapsuleFleetEntry["auditChain"],
  over: { name?: string; title?: string } = {},
): CapsuleFleetEntry {
  return {
    capsule: { name: over.name ?? "vm-x", title: over.title ?? "X", trust_state },
    spendBudget,
    auditChain,
  };
}

describe("homeView (Home fleet composition)", () => {
  it("is a pure composition — each row equals capsuleDetailView, in input order", () => {
    const fleet: CapsuleFleetEntry[] = [
      entry("cid-with-manifest-signature", { limit: 100, spent: 30, remaining: 70 }, verifiedChain, { name: "vm-a", title: "A" }),
      entry("local-dev", null, null, { name: "vm-b", title: "B" }),
    ];
    const v = homeView(fleet);
    assert.deepEqual(Object.keys(v).sort(), ["capsules", "needsAttention", "total"]);
    assert.equal(v.total, 2);
    // Order preserved (not reordered by "health"), each row is exactly the detail view-model.
    assert.deepEqual(v.capsules[0], capsuleDetailView(fleet[0].capsule, fleet[0].spendBudget, fleet[0].auditChain));
    assert.deepEqual(v.capsules[1], capsuleDetailView(fleet[1].capsule, fleet[1].spendBudget, fleet[1].auditChain));
    // No invented fleet-level roll-up verdict.
    assert.equal("overall" in v, false);
    assert.equal("allClear" in v, false);
  });

  it("needsAttention counts ONLY the unambiguously-wrong sub-states", () => {
    const fleet: CapsuleFleetEntry[] = [
      // wrong: unsigned trust
      entry("local-dev", { limit: 100, spent: 10, remaining: 90 }, verifiedChain, { name: "vm-unsigned" }),
      // wrong: exhausted spend (trust verified)
      entry("cid-with-manifest-signature", { limit: 5, spent: 5, remaining: 0 }, verifiedChain, { name: "vm-exhausted" }),
      // wrong: broken chain (trust verified)
      entry("cid-with-manifest-signature", { limit: 100, spent: 1, remaining: 99 }, brokenChain, { name: "vm-broken" }),
      // NOT wrong: verified + within budget + chain verified
      entry("cid-with-manifest-signature", { limit: 100, spent: 1, remaining: 99 }, verifiedChain, { name: "vm-clean" }),
    ];
    const v = homeView(fleet);
    assert.equal(v.needsAttention, 3, "three wrong capsules, the clean one excluded");
  });

  it("intermediate honest states (content_addressed / warning / absent) do NOT trip attention", () => {
    const fleet: CapsuleFleetEntry[] = [
      // content_addressed trust, near-limit (warning) spend, absent chain — all honest intermediates.
      entry("cid-without-manifest-signature", { limit: 100, spent: 85, remaining: 15 }, null, { name: "vm-mid" }),
    ];
    const v = homeView(fleet);
    // Sanity: the row really is in those intermediate states...
    assert.equal(v.capsules[0].trust, "content_addressed");
    assert.equal(v.capsules[0].custody.spend.state, "warning");
    assert.equal(v.capsules[0].custody.audit.state, "absent");
    // ...yet none of them is "wrong", so attention stays 0.
    assert.equal(v.needsAttention, 0);
    assert.equal(capsuleNeedsAttention(v.capsules[0]), false);
  });

  it("an empty fleet is 0 of 0 — absence is not reassurance", () => {
    const v = homeView([]);
    assert.deepEqual(v.capsules, []);
    assert.equal(v.total, 0);
    assert.equal(v.needsAttention, 0);
  });

  it("homeCapsules scopes to the user-facing set (drops provider/content infra)", () => {
    const catalog = [
      { name: "vm-agent", title: "Agent", trust_state: "local-dev", role: "app" },
      { name: "vm-owned", title: "Owned Video", trust_state: "local-dev", role: "viewer" },
      { name: "shell", title: "Shell", trust_state: "local-dev", role: "shell" },
      { name: "ai-provider", title: "Ai Provider", trust_state: "local-dev", role: "provider" },
      { name: "block", title: "Content Block", trust_state: "local-dev", role: "content" },
    ];
    const home = homeCapsules(catalog);
    assert.deepEqual(
      home.map((c) => c.name),
      ["vm-agent", "vm-owned", "shell"],
      "providers and content are dropped; app/viewer/shell kept in order",
    );
    // The predicate mirrors CapsuleRole::is_shell_launchable.
    assert.equal(isHomeCapsule("app"), true);
    assert.equal(isHomeCapsule("viewer"), true);
    assert.equal(isHomeCapsule("shell"), true);
    assert.equal(isHomeCapsule("provider"), false);
    assert.equal(isHomeCapsule("content"), false);
    assert.equal(isHomeCapsule("some-future-role"), false, "unknown role is not Home (fail-honest)");
  });

  it("a single wrong sub-state is enough — attention is OR over the three", () => {
    // Only the chain is broken; trust verified, budget fine. Still flagged.
    const v = homeView([entry("cid-with-manifest-signature", { limit: 100, spent: 1, remaining: 99 }, brokenChain)]);
    assert.equal(v.needsAttention, 1);
  });
});
