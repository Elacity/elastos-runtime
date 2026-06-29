/**
 * ESP v0 — the flywheel artifact (W7) conformance tests.
 *
 * Proves the consumer's signed receipt re-projects into the enterprise
 * containment-audit record, and that the evidence check is fail-closed: an
 * unsigned record fails Art 12 (no receipt → no provable act), and a high-risk
 * act executed without human consent fails Art 14.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { ESP_SCHEMA_TAGS } from "./esp_v0.js";
import type { AffordanceGrantReceiptV1 } from "./esp_v0.js";
import {
  AI_ACT_AUDIT_SCHEMA_V1,
  containmentEvidence,
  toAiActAuditRecord,
  type ChainAttestation,
  type ConsentContext,
} from "./ai_act_audit.js";

const verifiedChain: ChainAttestation = {
  verified: true,
  records: 42,
  signer: "deadbeefkey",
  error: null,
};
const brokenChain: ChainAttestation = {
  verified: false,
  records: 0,
  signer: "deadbeefkey",
  error: "audit tamper at seq 7: record_hash mismatch (content edited)",
};

function receipt(p: Partial<AffordanceGrantReceiptV1> = {}): AffordanceGrantReceiptV1 {
  return {
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
    ...p,
  };
}

const userConsent: ConsentContext = {
  approval: "user",
  principal_id: "did:ela:alice",
  request_id: "req-1",
  risk: "rights",
};

describe("the AI Act audit artifact (W7)", () => {
  it("maps the signed receipt + consent into the audit record", () => {
    const record = toAiActAuditRecord(userConsent, receipt());
    assert.equal(record.schema, AI_ACT_AUDIT_SCHEMA_V1);
    assert.equal(record.act.method_id, "play");
    assert.equal(record.act.resource, "elastos://rights/film-x");
    assert.equal(record.human_oversight.mechanism, "user-consent");
    assert.equal(record.human_oversight.required, true);
    assert.equal(record.human_oversight.principal, "did:ela:alice");
    assert.equal(record.record_keeping.signed, true);
  });

  it("a user-approved, signed act is provably contained (Art 12 + Art 14)", () => {
    const ev = containmentEvidence(toAiActAuditRecord(userConsent, receipt()));
    assert.equal(ev.article_12_met, true);
    assert.equal(ev.article_14_met, true);
    assert.equal(ev.contained, true);
  });

  it("an UNSIGNED record fails Art 12 — no receipt, no provable act", () => {
    const ev = containmentEvidence(toAiActAuditRecord(userConsent, receipt({ signature: "" })));
    assert.equal(ev.article_12_met, false);
    assert.equal(ev.contained, false);
  });

  it("a HIGH-RISK act executed WITHOUT human consent fails Art 14", () => {
    // A payment (high-risk) act that was runtime-policy approved — no human in loop.
    const automatedHighRisk: ConsentContext = {
      approval: "runtime_policy",
      principal_id: "did:ela:alice",
      request_id: "req-2",
      risk: "payment",
    };
    const record = toAiActAuditRecord(automatedHighRisk, receipt({ action: "execute" }));
    assert.equal(record.human_oversight.required, true, "high-risk requires a human");
    const ev = containmentEvidence(record);
    assert.equal(ev.article_14_met, false, "no human consent on a high-risk act is flagged");
    assert.equal(ev.contained, false);
  });

  it("a low-risk runtime-policy act with a signed record is contained (no human required)", () => {
    const lowRiskAuto: ConsentContext = {
      approval: "runtime_policy",
      principal_id: "did:ela:alice",
      request_id: "req-3",
      risk: "read",
    };
    const record = toAiActAuditRecord(lowRiskAuto, receipt({ method_id: "list", action: "read" }));
    assert.equal(record.human_oversight.required, false);
    const ev = containmentEvidence(record);
    assert.equal(ev.contained, true);
  });

  it("embeds a verified custody-chain attestation; the artifact is self-verifying", () => {
    const record = toAiActAuditRecord(userConsent, receipt(), verifiedChain);
    assert.equal(record.record_keeping.chain_attestation?.verified, true);
    assert.equal(record.record_keeping.chain_attestation?.records, 42);
    const ev = containmentEvidence(record);
    assert.equal(ev.chain_intact, true);
    assert.equal(ev.article_12_met, true);
    assert.equal(ev.contained, true);
  });

  it("a signed record on a BROKEN custody chain fails Art 12 (tamper-evidence compromised)", () => {
    const record = toAiActAuditRecord(userConsent, receipt(), brokenChain);
    assert.equal(record.record_keeping.signed, true, "the receipt itself is signed");
    const ev = containmentEvidence(record);
    assert.equal(ev.chain_intact, false);
    assert.equal(ev.article_12_met, false, "a tampered custody chain cannot back the record");
    assert.equal(ev.contained, false);
  });

  it("an absent attestation falls back to the signed-record check (no new failure)", () => {
    // Default (no chain arg) ⇒ null ⇒ memory-only / not-threaded; back-compatible.
    const record = toAiActAuditRecord(userConsent, receipt());
    assert.equal(record.record_keeping.chain_attestation, null);
    const ev = containmentEvidence(record);
    assert.equal(ev.chain_intact, true, "absence is not a failure");
    assert.equal(ev.contained, true);
  });
});
