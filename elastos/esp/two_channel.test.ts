/**
 * ESP v0 — two-channel + hero-act conformance tests.
 *
 * Proves, from real ESP fact shapes, the never-seen moment: a VERIFIED capsule
 * can read MORE dangerous than an UNSIGNED one — so you can refuse the thing the
 * green checkmark trained you to trust. Plus the hero dDRM act's safety: a
 * redemption receipt must attest the exact act that was requested.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { ESP_SCHEMA_TAGS } from "./esp_v0.js";
import type {
  AffordanceConsentPending,
  AffordanceReachView,
  ReachDescriptorV1,
  ValidateAndConsumeInput,
  ValidateAndConsumeOutput,
} from "./esp_v0.js";
import { blastRadius, hazardRank, trustMaterial, twoChannel } from "./two_channel.js";
import { consentPendingIsWellFormed, receiptMatchesRequest } from "./consent_act.js";

// ── fixtures (real ESP v0 shapes) ───────────────────────────────────────────
function reach(p: Partial<ReachDescriptorV1> = {}): ReachDescriptorV1 {
  return {
    schema: "elastos.reach.v1",
    egress: "none",
    isolation: "wasm",
    scope: "object",
    reversibility: "reversible",
    observed: true,
    ...p,
  };
}
function view(p: Partial<ReachDescriptorV1> = {}, understates = false): AffordanceReachView {
  return {
    interface_id: "iface.x",
    method_id: "m",
    risk: "read",
    reach: reach(p),
    declared_understates_reach: understates,
  };
}

describe("Channel 1 — trust-material (projected from the runtime verdict)", () => {
  it("maps signed verdicts to verified", () => {
    assert.equal(trustMaterial({ trust_state: "cid-with-manifest-signature" }), "verified");
    assert.equal(trustMaterial({ trust_state: "local-manifest-signature" }), "verified");
  });
  it("maps cid-only to content_addressed and local-dev to unsigned", () => {
    assert.equal(trustMaterial({ trust_state: "cid-without-manifest-signature" }), "content_addressed");
    assert.equal(trustMaterial({ trust_state: "local-dev" }), "unsigned");
  });
  it("fails honest: an unknown verdict reads as unsigned, never over-trusted", () => {
    assert.equal(trustMaterial({ trust_state: "some-future-state" }), "unsigned");
  });
});

describe("Channel 2 — blast-radius (projected from core-computed reach)", () => {
  it("a sandboxed read is cool and complete", () => {
    const b = blastRadius(reach());
    assert.equal(b.level, "cool");
    assert.equal(b.incomplete, false);
  });
  it("open egress + system scope + irreversible is hot", () => {
    const b = blastRadius(reach({ egress: "open", scope: "system", reversibility: "one_way" }));
    assert.equal(b.level, "hot");
  });
  it("a leashed (allowlisted) egress reads cooler than an open one — the W1 point", () => {
    const leashed = blastRadius(reach({ egress: "allowlisted" }));
    const open = blastRadius(reach({ egress: "open" }));
    assert.ok(
      hazardRank(leashed.level) < hazardRank(open.level),
      "allowlisted egress must rank below open egress",
    );
  });
  it("an unobserved dimension renders the halo incomplete, not falsely cool", () => {
    assert.equal(blastRadius(reach({ observed: false })).incomplete, true);
  });
});

describe("The two-channel object — the never-seen moment", () => {
  it("a VERIFIED capsule can read MORE dangerous than an UNSIGNED one", () => {
    const verifiedFar = twoChannel("verified", view({ egress: "open", scope: "system", reversibility: "one_way" }));
    const unsignedContained = twoChannel("unsigned", view());

    assert.equal(verifiedFar.blast.level, "hot");
    assert.equal(unsignedContained.blast.level, "cool");
    assert.ok(
      hazardRank(verifiedFar.blast.level) > hazardRank(unsignedContained.blast.level),
      "a verified+far affordance is more dangerous than an unsigned+contained one",
    );
    // ...and the UI lets you refuse the thing the green check trained you to trust.
    assert.equal(verifiedFar.refuseTrained, true);
    assert.equal(unsignedContained.refuseTrained, false);
  });

  it("surfaces the declared-understates-reach contradiction", () => {
    const lying = twoChannel("verified", view({ egress: "open" }, true));
    assert.equal(lying.declaredUnderstatesReach, true);
  });
});

describe("The hero dDRM consent act (W2 shapes)", () => {
  const pending: AffordanceConsentPending = {
    schema: ESP_SCHEMA_TAGS.affordanceConsentPending,
    status: "approval_pending",
    request_id: "req-1",
    resource: "elastos://rights/film-x",
    action: "execute",
    risk: "rights",
    approval: "user",
    capsule: "vm-player",
    interface: "iface.play",
    method: "play",
    principal_id: "did:ela:alice",
  };

  it("recognises a well-formed consent request (202)", () => {
    assert.ok(consentPendingIsWellFormed(pending));
    assert.ok(!consentPendingIsWellFormed({ ...pending, request_id: "" }));
  });

  it("the receipt must attest the SAME act that was requested", () => {
    const request: ValidateAndConsumeInput = {
      token: "base64-token",
      method_id: "play",
      resource: "elastos://rights/film-x",
      action: "execute",
      input: { track: "film-x" },
    };
    const consumed: ValidateAndConsumeOutput = {
      status: "consumed",
      receipt: {
        schema: ESP_SCHEMA_TAGS.affordanceReceipt,
        capsule: "vm-player",
        method_id: "play",
        input_hash: "abc123",
        resource: "elastos://rights/film-x",
        action: "execute",
        token_id: "tok-1",
        redeemed_at: 0,
        signer: "deadbeef",
        signature: "sig",
      },
    };
    assert.ok(receiptMatchesRequest(request, consumed), "a matching receipt renders the act as done");

    // A receipt for a DIFFERENT method must NOT be rendered as the act being done.
    const wrongMethod: ValidateAndConsumeOutput = {
      ...consumed,
      receipt: { ...consumed.receipt, method_id: "delete" },
    };
    assert.ok(!receiptMatchesRequest(request, wrongMethod));
  });
});
