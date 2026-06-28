/**
 * ESP v0 — the flywheel's first turn (W7): the signed receipt, re-projected as
 * the enterprise containment-audit artifact.
 *
 * The SAME `AffordanceGrantReceiptV1` that delights the consumer IS the
 * compliance evidence a regulator/insurer forces. This module maps a redemption
 * (the consent context + the signed receipt) onto the controls an auditor checks:
 *
 *   - EU AI Act **Art 12** (record-keeping): an automatic, tamper-evident record
 *     of the agent action — the ed25519-signed receipt.
 *   - EU AI Act **Art 14** (human oversight): a high-risk or user-approval act
 *     required explicit human consent before it executed.
 *
 * Pure projection — it asserts the evidence is PRESENT (signed record + matching
 * oversight); the cryptographic verification is the runtime's job.
 */

import type { AffordanceConsentPending, AffordanceGrantReceiptV1 } from "./esp_v0.js";

export const AI_ACT_AUDIT_SCHEMA_V1 = "elastos.audit.ai-act.v1";

/** Risk classes that require a human in the loop regardless of approval mode. */
const HIGH_RISK: ReadonlySet<string> = new Set(["payment", "rights", "actuator", "privileged"]);

/** The consent context an audit record needs (a subset of the 202 consent fact). */
export type ConsentContext = Pick<
  AffordanceConsentPending,
  "approval" | "principal_id" | "request_id" | "risk"
>;

export interface AiActAuditRecordV1 {
  schema: typeof AI_ACT_AUDIT_SCHEMA_V1;
  /** What the agent did, in the user's name (from the signed receipt). */
  act: {
    capsule: string;
    method_id: string;
    resource: string;
    action: string;
    input_hash: string;
  };
  /** Human oversight (EU AI Act Art 14). */
  human_oversight: {
    required: boolean;
    mechanism: "user-consent" | "runtime-policy";
    principal: string;
    request_id: string;
  };
  /** Record-keeping (EU AI Act Art 12): the tamper-evident signed proof. */
  record_keeping: {
    token_id: string;
    signed: boolean;
    signer: string;
    signature: string;
    recorded_at: unknown;
  };
  /** Plain-language mapping of each fact to the control it satisfies. */
  controls: {
    article_12_logging: string;
    article_14_oversight: string;
    soc2_for_agents: string;
  };
}

/**
 * Project a redemption (consent context + signed receipt) into a containment-audit
 * record. `required` human oversight is true for a user-approval act OR any
 * high-risk class; `signed` is true only when both a signer and a signature are
 * present on the receipt.
 */
export function toAiActAuditRecord(
  consent: ConsentContext,
  receipt: AffordanceGrantReceiptV1,
): AiActAuditRecordV1 {
  const requiresHuman = consent.approval === "user" || HIGH_RISK.has(consent.risk);
  const signed = receipt.signer.length > 0 && receipt.signature.length > 0;
  return {
    schema: AI_ACT_AUDIT_SCHEMA_V1,
    act: {
      capsule: receipt.capsule,
      method_id: receipt.method_id,
      resource: receipt.resource,
      action: receipt.action,
      input_hash: receipt.input_hash,
    },
    human_oversight: {
      required: requiresHuman,
      mechanism: consent.approval === "user" ? "user-consent" : "runtime-policy",
      principal: consent.principal_id,
      request_id: consent.request_id,
    },
    record_keeping: {
      token_id: receipt.token_id,
      signed,
      signer: receipt.signer,
      signature: receipt.signature,
      recorded_at: receipt.redeemed_at,
    },
    controls: {
      article_12_logging:
        "EU AI Act Art 12 — automatic, tamper-evident record of the agent action (the ed25519-signed receipt).",
      article_14_oversight:
        "EU AI Act Art 14 — human-in-the-loop: a high-risk or user-approval act required explicit human consent before execution.",
      soc2_for_agents:
        "Containment evidence: the act was gated, scoped (single-use, bound), and signed — provably stayed in its lane.",
    },
  };
}

export interface ContainmentEvidence {
  /** A tamper-evident signed record exists (Art 12). */
  article_12_met: boolean;
  /** Where human oversight was required, a human actually consented (Art 14). */
  article_14_met: boolean;
  /** Both controls satisfied — the act is provably contained. */
  contained: boolean;
}

/**
 * Whether the audit record demonstrates containment. Fail-closed at the audit
 * layer: an unsigned record fails Art 12 (no receipt → no provable act), and a
 * high-risk/user-approval act executed without human consent fails Art 14.
 */
export function containmentEvidence(record: AiActAuditRecordV1): ContainmentEvidence {
  const article12Met = record.record_keeping.signed;
  const article14Met =
    !record.human_oversight.required || record.human_oversight.mechanism === "user-consent";
  return {
    article_12_met: article12Met,
    article_14_met: article14Met,
    contained: article12Met && article14Met,
  };
}
